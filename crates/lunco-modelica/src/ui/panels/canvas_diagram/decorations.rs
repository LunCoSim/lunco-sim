//! Diagram-level decoration layer + AST→decoration emission.
//!
//! Each diagram has class-level `Diagram(graphics={...})` and
//! `Documentation` annotations that aren't attached to any component.
//! `DiagramDecorationLayer` paints them as a background layer behind
//! every node. `emit_diagram_decorations` walks the parent class AST
//! and produces the layer's primitives via `paint_graphics` /
//! text-node materialisation.

use super::BackgroundDiagramHandle;

/// Paints the target class's `Diagram(graphics={...})` annotation as
/// canvas background — the red labelled rectangles, text callouts,
/// and accent lines MSL example diagrams carry for reader orientation
/// (the PID example's "reference speed generation" / "PI controller"
/// / "plant" regions are the canonical case). Holds an
/// `Arc<RwLock<…>>` handle so the projector can push a new class's
/// graphics in without reaching into the canvas's layer list.
pub(super) struct DiagramDecorationLayer {
    pub(super) data: BackgroundDiagramHandle,
}

impl lunco_canvas::Layer for DiagramDecorationLayer {
    fn name(&self) -> &'static str {
        "modelica.diagram_decoration"
    }
    fn draw(
        &mut self,
        ctx: &mut lunco_canvas::visual::DrawCtx,
        _scene: &lunco_canvas::Scene,
        _selection: &lunco_canvas::Selection,
    ) {
        let Ok(guard) = self.data.read() else { return };
        // `_plot_nodes` is unused here: this decoration layer only
        // paints `graphics` as background. Plot tiles emit as scene
        // Nodes via `emit_diagram_decorations`, drawn separately by
        // the canvas node-visual pipeline.
        let Some((coord_system, graphics, _plot_nodes)) = guard.as_ref() else {
            return;
        };
        // Map the coordinate system's extent (Modelica +Y up) to the
        // canvas world rect (+Y down) by flipping Y. Our node
        // placements already live in this flipped space, so the
        // decoration lines up with the nodes natively.
        let ext = coord_system.extent;
        let world_min_x = (ext.p1.x.min(ext.p2.x)) as f32;
        let world_max_x = (ext.p1.x.max(ext.p2.x)) as f32;
        let world_min_y = -(ext.p1.y.max(ext.p2.y) as f32);
        let world_max_y = -(ext.p1.y.min(ext.p2.y) as f32);
        let world_rect = lunco_canvas::Rect::from_min_max(
            lunco_canvas::Pos::new(world_min_x, world_min_y),
            lunco_canvas::Pos::new(world_max_x, world_max_y),
        );
        let screen_rect_canvas = ctx
            .viewport
            .world_rect_to_screen(world_rect, ctx.screen_rect);
        let screen_rect = bevy_egui::egui::Rect::from_min_max(
            bevy_egui::egui::pos2(screen_rect_canvas.min.x, screen_rect_canvas.min.y),
            bevy_egui::egui::pos2(screen_rect_canvas.max.x, screen_rect_canvas.max.y),
        );
        // Filter out items that have a corresponding scene Node
        // (Text → editable label). Plot tiles live in a separate
        // vendor annotation now and never appear in `graphics`, so
        // they don't need filtering here. Painting Text as well
        // would double-render: the scene Node already paints itself
        // via its `NodeVisual` and the decoration would sit on top
        // with stale text. Other graphics (Rectangle / Line /
        // Polygon / Ellipse / Bitmap) stay as background decoration.
        use crate::annotations::GraphicItem;
        let decoration: Vec<GraphicItem> = graphics
            .iter()
            .filter(|g| !matches!(g, GraphicItem::Text(_)))
            .cloned()
            .collect();
        crate::icon_paint::paint_graphics(
            ctx.ui.painter(),
            screen_rect,
            *coord_system,
            &decoration,
        );
    }
}

/// Extract the `Diagram(graphics={...})` annotation for the target
/// class — full-qualified drill-in target, or the first non-package
/// class when no drill-in is active. Used by the background
/// decoration layer to paint MSL-style diagram callouts (labelled
/// regions, accent text) behind the nodes.
/// Emit canvas Nodes for every interactive item in the active
/// class's diagram. Two sources, intentionally split:
///
///   * `Text` entries in `Diagram(graphics=…)` → `lunco.modelica.text`
///     (editable label). Standard Modelica; OMEdit renders these too.
///   * `LunCoAnnotations.PlotNode(...)` records in
///     `annotation(__LunCo(plotNodes=…))` → `lunco.viz.plot`
///     (live signal tile). Lunica-only feature: each tile binds a
///     runtime `signal=` and paints a real-time graph on top of the
///     placeholder `Rectangle` the source carries for OMEdit's
///     benefit. OMEdit shows the rectangle; Lunica covers it with
///     the live plot. Same source, two valid renderings.
///
/// Each emitted Node carries a stable `origin` marker derived from
/// the annotation's position in the source (`text:<idx>` or
/// `plot:<idx>:<signal>`) so the canvas-edit pipeline recognises it
/// as source-backed and the carry-over filter doesn't double-insert.
/// Returns the set of emitted origin keys.
pub(super) fn emit_diagram_decorations(
    scene: &mut lunco_canvas::scene::Scene,
    graphics: &[crate::annotations::GraphicItem],
    plot_nodes: &[crate::annotations::LunCoPlotNode],
    doc_id: Option<lunco_doc::DocumentId>,
) -> std::collections::HashSet<String> {
    use crate::annotations::GraphicItem;
    let mut origins: std::collections::HashSet<String> = Default::default();
    let mut text_idx: usize = 0;
    for item in graphics.iter() {
        if let GraphicItem::Text(t) = item {
            // Editable label. Strip surrounding quotes the parser
            // left on `textString` so the visual sees the raw
            // string. Skip `%name` / `%class` substitutions and
            // empty strings — those are MSL conventions for
            // icon-internal Text and aren't meaningful as Diagram
            // callouts.
            let raw = t.text_string.trim_matches('"');
            if raw.is_empty() || raw.starts_with('%') {
                text_idx += 1;
                continue;
            }
            let payload = crate::ui::text_node::TextNodeData {
                text: raw.to_string(),
                font_size: t.font_size,
                color: t.text_color.map(|c| [c.r, c.g, c.b]),
            };
            let data: lunco_canvas::NodeData = std::sync::Arc::new(payload);
            let origin = format!("text:{text_idx}");
            origins.insert(origin.clone());
            let id = scene.alloc_node_id();
            // Same Y-flip + corner-normalize the plot path uses.
            // Modelica `extent` is +Y up, canvas world is +Y down.
            let x1 = t.extent.p1.x as f32;
            let x2 = t.extent.p2.x as f32;
            let y1 = -(t.extent.p1.y as f32);
            let y2 = -(t.extent.p2.y as f32);
            let rect = lunco_canvas::Rect::from_min_max(
                lunco_canvas::Pos::new(x1.min(x2), y1.min(y2)),
                lunco_canvas::Pos::new(x1.max(x2), y1.max(y2)),
            );
            scene.insert_node(lunco_canvas::scene::Node {
                id,
                rect,
                kind: crate::ui::text_node::TEXT_NODE_KIND.into(),
                data,
                ports: Vec::new(),
                label: String::new(),
                origin: Some(origin),
                resizable: true,
                visual_rect: None,
            });
            text_idx += 1;
        }
    }
    // Live plot tiles — emitted *after* texts so they sit on top in
    // canvas draw order. The tile visual is opaque and covers the
    // placeholder Rectangle/Text from `graphics`, giving Lunica
    // users a real-time graph where OMEdit shows a labelled region.
    for (idx, plot) in plot_nodes.iter().enumerate() {
        // Source-backed tiles use the Doc binding: resolved to the
        // document's current sim entity every frame via the
        // snapshot's `doc_to_entity` table. Falls back to a Pinned
        // unbound state (`entity = 0`) if there's no document
        // context — defensive, doesn't happen in practice since
        // projection is always doc-scoped.
        let binding = match doc_id {
            Some(d) => lunco_viz::kinds::canvas_plot_node::PlotBinding::Doc { doc_id: d.raw() },
            None => lunco_viz::kinds::canvas_plot_node::PlotBinding::Pinned { entity: 0 },
        };
        let payload = lunco_viz::kinds::canvas_plot_node::PlotNodeData {
            binding,
            signal_path: plot.signal.clone(),
            title: plot.title.clone(),
        };
        let data: lunco_canvas::NodeData = std::sync::Arc::new(payload);
        let origin = format!("plot:{idx}:{}", plot.signal);
        origins.insert(origin.clone());
        let id = scene.alloc_node_id();
        // Modelica `extent={{x1,y1},{x2,y2}}` is +Y up and doesn't
        // enforce corner ordering. Flip Y per corner, normalise so
        // `from_min_max` sees `min < max`, otherwise the tile lands
        // far above the icons or shrinks to a zero-area rect.
        let x1 = plot.extent.p1.x as f32;
        let x2 = plot.extent.p2.x as f32;
        let y1 = -(plot.extent.p1.y as f32);
        let y2 = -(plot.extent.p2.y as f32);
        let rect = lunco_canvas::Rect::from_min_max(
            lunco_canvas::Pos::new(x1.min(x2), y1.min(y2)),
            lunco_canvas::Pos::new(x1.max(x2), y1.max(y2)),
        );
        scene.insert_node(lunco_canvas::scene::Node {
            id,
            rect,
            kind: lunco_viz::kinds::canvas_plot_node::PLOT_NODE_KIND.into(),
            data,
            ports: Vec::new(),
            label: String::new(),
            origin: Some(origin),
            resizable: true,
            visual_rect: None,
        });
    }
    origins
}

/// Resolve the canvas background decorations for the target class:
/// the standard `Diagram(graphics=…)` shapes AND the LunCo
/// `__LunCo(plotNodes=…)` live tiles, extracted *independently*.
///
/// The two are orthogonal annotations (see [`crate::annotations::Diagram`]):
/// a model can have diagram graphics, plot tiles, both, or neither.
/// We therefore never gate one on the other — a pure behaviour model
/// with only plot tiles (no `Diagram` block) still returns its tiles.
/// Returns `None` only when the class isn't found or carries neither.
pub(super) fn diagram_annotation_for_target(
    ast: &rumoca_compile::parsing::ast::StoredDefinition,
    target: Option<&str>,
) -> Option<(
    crate::annotations::CoordinateSystem,
    Vec<crate::annotations::GraphicItem>,
    Vec<crate::annotations::LunCoPlotNode>,
)> {
    // Route through the canonical AST class lookup
    // `crate::diagram::find_class_by_qualified_name`. It already
    // handles within-clause stripping correctly — at segment
    // boundaries, not raw string boundaries. The earlier local
    // `walk_qualified` here used `.unwrap_or(rest)` after stripping
    // a non-dot suffix, which corrupted targets when the within
    // prefix happened to be a *string* prefix of the next segment
    // (the bug that hid the diagram for `Duplicate to edit` of any
    // class whose copy name was `<OriginalCopy>` — the within clause
    // `AnnotatedRocketStage` is a string prefix of
    // `AnnotatedRocketStageCopy`, so the strip mangled the target
    // to `Copy.RocketStage` and the walk found nothing).
    // For `None` targets fall back to the first non-package class,
    // matching the workbench's default active-class picker.
    let class = if let Some(qualified) = target {
        crate::diagram::find_class_by_qualified_name(ast, qualified)
    } else {
        use rumoca_compile::parsing::ClassType;
        ast.classes
            .iter()
            .find(|(_, c)| !matches!(c.class_type, ClassType::Package))
            .map(|(_, c)| c)
    };
    let class = class?;
    // Two orthogonal sources, extracted independently so neither
    // gates the other:
    //   * standard `Diagram(graphics=…)` → background shapes
    //   * `__LunCo(plotNodes=…)`          → live signal tiles
    let diagram = crate::annotations::extract_diagram(&class.annotation);
    let plot_nodes = crate::annotations::extract_lunco_plot_nodes(&class.annotation);
    if diagram.is_none() && plot_nodes.is_empty() {
        return None;
    }
    let (coordinate_system, graphics) = diagram
        .map(|d| (d.coordinate_system, d.graphics))
        .unwrap_or_default();
    Some((coordinate_system, graphics, plot_nodes))
}

// `walk_qualified` deleted: was a near-duplicate of
// `crate::diagram::find_class_by_qualified_name` and silently
// disagreed with it on the within-clause strip. Two sources of
// truth for the same lookup is how the duplicate-renders-nothing
// bug shipped. Use the canonical helper.
