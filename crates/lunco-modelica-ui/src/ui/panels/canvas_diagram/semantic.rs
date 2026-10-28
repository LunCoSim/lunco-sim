//! Frozen native diagram previews prepared off the UI thread.
use super::{edge::ConnectionEdgeData, node::IconNodeData};
use crate::ui::panels::canvas_projection::DiagramAutoLayoutSettings;
use bevy_egui::egui;
use lunco_canvas::{DrawCtx, Layer, Pos, Rect, Scene, Selection, Viewport};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
pub(super) struct ResidentSource {
    pub document: lunco_doc::DocumentId,
    pub source: Arc<str>,
    pub ast: Arc<rumoca_compile::parsing::ast::StoredDefinition>,
}
type Sources = std::collections::HashMap<
    String,
    (
        Arc<rumoca_compile::parsing::ast::StoredDefinition>,
        Arc<str>,
    ),
>;
const MAX_DEPTH: usize = 8;
const REVEAL_SIZE: f32 = 320.0;

pub(super) struct SemanticDiagram {
    scene: Scene,
    bounds: Rect,
    source: String,
    diagnostic: Option<String>,
    layers: Mutex<Vec<Box<dyn Layer>>>,
}
impl std::fmt::Debug for SemanticDiagram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SemanticDiagram")
            .field("nodes", &self.scene.node_count())
            .field("diagnostic", &self.diagnostic)
            .finish()
    }
}
pub(super) fn prepare(
    scene: &mut Scene,
    engine: &mut crate::engine::ModelicaEngine,
    max_nodes: usize,
    layout: &DiagramAutoLayoutSettings,
    cancel: &AtomicBool,
    documents: &[ResidentSource],
) -> Result<(), String> {
    let sources: Sources = documents
        .iter()
        .map(|d| {
            (
                engine.uri_for(d.document),
                (d.ast.clone(), d.source.clone()),
            )
        })
        .collect();
    let mut remaining = max_nodes.saturating_sub(scene.node_count());
    enrich(
        scene,
        engine,
        layout,
        cancel,
        &mut remaining,
        &mut Vec::new(),
        "",
        &sources,
    )
}
fn enrich(
    scene: &mut Scene,
    engine: &mut crate::engine::ModelicaEngine,
    layout: &DiagramAutoLayoutSettings,
    cancel: &AtomicBool,
    remaining: &mut usize,
    ancestry: &mut Vec<String>,
    prefix: &str,
    sources: &Sources,
) -> Result<(), String> {
    let nodes: Vec<_> = scene
        .nodes()
        .filter_map(|(id, n)| {
            n.data
                .downcast_ref::<IconNodeData>()
                .map(|d| (*id, d.clone()))
        })
        .collect();
    for (id, mut data) in nodes {
        if cancel.load(Ordering::Relaxed) {
            return Err("Diagram preparation cancelled".into());
        }
        data.value_instance = if prefix.is_empty() {
            data.instance_name.clone()
        } else {
            format!("{prefix}.{}", data.instance_name)
        };
        let definition = engine
            .session_mut()
            .class_definition_query(&data.qualified_type)
            .cloned();
        let Some(definition) = definition else {
            continue;
        };
        if !matches!(
            definition.class_type,
            lunco_modelica_ast::ClassType::Model
                | lunco_modelica_ast::ClassType::Block
                | lunco_modelica_ast::ClassType::Class
        ) {
            continue;
        }
        let mut diagnostic = if ancestry.contains(&data.qualified_type) {
            Some("Recursive class: double-click to inspect this definition".to_string())
        } else if ancestry.len() >= MAX_DEPTH || *remaining == 0 {
            Some("Preview limit reached: double-click to inspect this definition".to_string())
        } else {
            None
        };
        // Rumoca owns class-to-document identity. A nested class may live in a different file.
        let owner_uri =
            crate::diagram::class_source_uri(engine.session_mut(), &data.qualified_type);
        let mut child = Scene::new();
        let mut excerpt = String::new();
        if diagnostic.is_none() {
            let owner = owner_uri.ok().and_then(|uri| {
                if let Some(resident) = sources.get(&uri) {
                    return Some(resident.clone());
                }
                let session = engine.session_mut();
                let ast = session.get_document(&uri)?.parsed()?.clone();
                Some((Arc::new(ast), session.document_source_text(&uri)?))
            });
            if let Some((ast, source)) = owner {
                // Complete parser span preserves both equations and algorithms.
                excerpt = source
                    .get(definition.location.start as usize..definition.location.end as usize)
                    .ok_or_else(|| format!("Invalid source span for {}", data.qualified_type))?
                    .to_string();
                let snapshot = engine.diagram_snapshot(
                    &data.qualified_type,
                    crate::diagram::DiagramScope::ComponentGraph,
                )?;
                match crate::ui::panels::canvas_projection::import_model_to_diagram_from_ast(
                    ast.clone(),
                    Arc::new(snapshot),
                    &source,
                    *remaining,
                    layout,
                ) {
                    Ok(diagram) => {
                        child = super::projection::project_scene(&diagram).0;
                        if child.node_count() > *remaining {
                            diagnostic = Some(
                                "Preview node limit reached; open this component to inspect".into(),
                            );
                            child = Scene::new();
                        } else {
                            *remaining -= child.node_count();
                            ancestry.push(data.qualified_type.clone());
                            enrich(
                                &mut child,
                                engine,
                                layout,
                                cancel,
                                remaining,
                                ancestry,
                                &data.value_instance,
                                sources,
                            )?;
                            ancestry.pop();
                            if let Some((_, graphics, plots)) =
                                super::decorations::diagram_annotation_for_target(
                                    &ast,
                                    Some(&data.qualified_type),
                                )
                            {
                                super::decorations::emit_diagram_decorations(
                                    &mut child, &graphics, &plots, None,
                                );
                            }
                        }
                    }
                    Err(error) => diagnostic = Some(error),
                }
            } else {
                diagnostic = Some(format!("Source for {} is not loaded", data.qualified_type));
            }
        }
        let bounds = child
            .bounds()
            .unwrap_or_else(|| Rect::from_min_size(Pos::new(0.0, 0.0), 400.0, 200.0));
        let registry = Arc::new(super::build_registry());
        data.semantic = Some(Arc::new(SemanticDiagram {
            scene: child,
            bounds,
            source: excerpt,
            diagnostic,
            layers: Mutex::new(vec![
                Box::new(lunco_canvas::EdgesLayer::new(registry.clone())),
                Box::new(lunco_canvas::NodesLayer::new(registry)),
            ]),
        }));
        if let Some(node) = scene.node_mut(id) {
            node.data = Arc::new(data);
        }
    }
    if !prefix.is_empty() {
        let edges: Vec<_> = scene
            .edges()
            .filter_map(|(id, e)| {
                e.data
                    .downcast_ref::<ConnectionEdgeData>()
                    .map(|d| (*id, d.clone()))
            })
            .collect();
        for (id, mut data) in edges {
            data.source_path = format!("{prefix}.{}", data.source_path);
            data.target_path = format!("{prefix}.{}", data.target_path);
            if let Some(edge) = scene.edge_mut(id) {
                edge.data = Arc::new(data);
            }
        }
    }
    Ok(())
}
impl SemanticDiagram {
    pub(super) fn draw(
        &self,
        ctx: &mut DrawCtx,
        rect: egui::Rect,
        path: &str,
        selected: bool,
    ) -> bool {
        let enabled = ctx
            .ui
            .ctx()
            .data(|d| d.get_temp::<bool>(egui::Id::new("modelica.semantic_zoom")))
            .unwrap_or(false);
        if !enabled || rect.width().min(rect.height()) < REVEAL_SIZE {
            return false;
        }
        let theme = super::theme::canvas_theme_from_ctx(ctx.ui.ctx());
        let clip = ctx.ui.clip_rect();
        let painter = ctx.ui.painter().with_clip_rect(clip.intersect(rect));
        painter.rect_filled(rect, 4.0, theme.card_fill);
        painter.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(if selected { 2.0 } else { 1.0 }, theme.select_stroke),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.min + egui::vec2(10.0, 8.0),
            egui::Align2::LEFT_TOP,
            path,
            egui::FontId::proportional(15.0),
            theme.node_label,
        );
        let inner = egui::Rect::from_min_max(
            rect.min + egui::vec2(14.0, 34.0),
            rect.max - egui::vec2(14.0, 14.0),
        );
        if let Some(message) = &self.diagnostic {
            painter.text(
                inner.min,
                egui::Align2::LEFT_TOP,
                message,
                egui::FontId::proportional(14.0),
                theme.warning_stroke,
            );
        } else if !self.scene.nodes().any(|(_, n)| {
            n.data
                .downcast_ref::<IconNodeData>()
                .is_some_and(|d| d.semantic.is_some())
        }) {
            let lines = self.source.lines().count().max(1) as f32;
            let cols = self
                .source
                .lines()
                .map(|l| l.chars().count())
                .max()
                .unwrap_or(1)
                .max(1) as f32;
            let size = (inner.height() / (lines * 1.25)).min(inner.width() / (cols * 0.62));
            let galley = lunco_theme::presentation::diagram_text_galley(
                &painter,
                self.source.clone(),
                egui::FontId::monospace(size),
                theme.node_label,
            );
            painter.galley(inner.min, galley, theme.node_label);
        } else {
            let mut viewport = Viewport::default();
            viewport.center = self.bounds.center();
            viewport.zoom = (inner.width() / (self.bounds.width() + 24.0))
                .min(inner.height() / (self.bounds.height() + 24.0));
            viewport.target_center = viewport.center;
            viewport.target_zoom = viewport.zoom;
            let screen_rect = Rect::from_min_max(
                Pos::new(inner.min.x, inner.min.y),
                Pos::new(inner.max.x, inner.max.y),
            );
            let mut child_ui = ctx
                .ui
                .new_child(egui::UiBuilder::new().id_salt(path).max_rect(inner));
            child_ui.disable();
            // Disabled interaction must not dim an engineering inspection.
            child_ui.set_opacity(1.0);
            child_ui.set_clip_rect(clip.intersect(inner));
            let mut nested = DrawCtx {
                ui: &mut child_ui,
                viewport: &viewport,
                screen_rect,
                time: ctx.time,
                show_edges: ctx.show_edges,
                extras: ctx.extras,
            };
            if let Ok(mut layers) = self.layers.lock() {
                for layer in layers.iter_mut() {
                    layer.draw(&mut nested, &self.scene, &Selection::default());
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn resident(
        engine: &mut crate::engine::ModelicaEngine,
        id: u64,
        source: &str,
    ) -> ResidentSource {
        let document = lunco_doc::DocumentId::new(id);
        let ast = rumoca_phase_parse::parse_to_ast(source, &engine.uri_for(document)).unwrap();
        engine.upsert_document_with_ast(document, ast.clone());
        ResidentSource {
            document,
            source: Arc::from(source),
            ast: Arc::new(ast),
        }
    }
    #[test]
    fn semantic_zoom_large_text_keeps_authored_size_without_glyph_overflow() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let galley = lunco_theme::presentation::diagram_text_galley(
                ui.painter(),
                "Process Ω — equation".into(),
                egui::FontId::proportional(100_000.0),
                egui::Color32::WHITE,
            );
            assert!(galley.size().x.is_finite());
            assert!(galley.size().y > 50_000.0);
            ui.painter()
                .galley(egui::Pos2::ZERO, galley, egui::Color32::WHITE);
        });
        output.textures_delta.clear();
    }
    #[test]
    fn semantic_zoom_projects_inherited_cross_file_hierarchy_and_scoped_connections() {
        let mut engine = crate::engine::ModelicaEngine::new();
        let parts = resident(
            &mut engine,
            1,
            r#"
package Parts
  connector Signal = Real;
  model Gain
    Signal u; Signal y;
  equation
    y=2*u;
  end Gain;
  model Pair
    Gain first annotation(Placement(transformation(extent={{-80,-20},{-40,20}})));
    Gain second annotation(Placement(transformation(extent={{40,-20},{80,20}})));
  equation
    connect(first.y,second.u);
  end Pair;
  model Base
    Pair unit;
  end Base;
end Parts;
"#,
        );
        let source = "model Derived extends Parts.Base; end Derived;";
        let root = resident(&mut engine, 2, source);
        let layout = DiagramAutoLayoutSettings::default();
        let snapshot = engine
            .diagram_snapshot("Derived", crate::diagram::DiagramScope::ComponentGraph)
            .unwrap();
        let ast = root.ast.clone();
        let sources = [parts, root];
        let diagram = crate::ui::panels::canvas_projection::import_model_to_diagram_from_ast(
            ast,
            Arc::new(snapshot),
            source,
            100,
            &layout,
        )
        .unwrap();
        let mut scene = super::super::projection::project_scene(&diagram).0;
        let cancel = AtomicBool::new(false);
        prepare(&mut scene, &mut engine, 100, &layout, &cancel, &sources).unwrap();
        let unit = scene
            .nodes()
            .find_map(|(_, n)| n.data.downcast_ref::<IconNodeData>())
            .unwrap();
        assert_eq!(unit.value_instance, "unit");
        let pair = unit.semantic.as_ref().unwrap();
        assert!(pair.diagnostic.is_none(), "{:?}", pair.diagnostic);
        assert_eq!(pair.scene.edge_count(), 1);
        let edge = pair
            .scene
            .edges()
            .next()
            .unwrap()
            .1
            .data
            .downcast_ref::<ConnectionEdgeData>()
            .unwrap();
        assert_eq!(edge.source_path, "unit.first.y");
        let first = pair
            .scene
            .nodes()
            .filter_map(|(_, n)| n.data.downcast_ref::<IconNodeData>())
            .find(|d| d.instance_name == "first")
            .unwrap();
        assert_eq!(first.value_instance, "unit.first");
        assert!(first.semantic.as_ref().unwrap().source.contains("y=2*u"));
        // A configured cap is visible at the node; cancellation rejects the job.
        prepare(&mut scene, &mut engine, 1, &layout, &cancel, &sources).unwrap();
        let limited = scene
            .nodes()
            .find_map(|(_, n)| n.data.downcast_ref::<IconNodeData>())
            .unwrap();
        assert!(
            limited
                .semantic
                .as_ref()
                .unwrap()
                .diagnostic
                .as_ref()
                .unwrap()
                .contains("limit")
        );
        cancel.store(true, Ordering::Relaxed);
        assert!(prepare(&mut scene, &mut engine, 100, &layout, &cancel, &sources).is_err());
        // Inherited equations are retained even when the root has none.
        engine
            .session_mut()
            .add_document(
                "derived-pair.mo",
                "model DerivedPair extends Parts.Pair; end DerivedPair;",
            )
            .unwrap();
        let inherited = engine
            .diagram_snapshot("DerivedPair", crate::diagram::DiagramScope::ComponentGraph)
            .unwrap();
        assert_eq!(inherited.equations.len(), 1);
    }
}
