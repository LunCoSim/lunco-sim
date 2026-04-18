//! Modelica diagram, rendered via `lunco-canvas`.
//!
//! A parallel path to the egui-snarl-backed `diagram.rs`, kept
//! alongside until the canvas version covers every snarl feature
//! we actually use. Users see both as tabs and can compare.
//!
//! # Pipeline
//!
//! ```text
//!   ModelicaDocument (AST)                        (lunco-doc)
//!           │
//!           ▼
//!   VisualDiagram  (existing intermediate)        (lunco-modelica)
//!           │  project_scene()
//!           ▼
//!   lunco_canvas::Scene   →  Canvas   →  egui
//!           ▲                  │
//!           └──── SceneEvent ──┘      → (future) DocumentOp back to ModelicaDocument
//! ```
//!
//! # What's in B2
//!
//! - Read-side projector: `VisualDiagram → Scene` (one-shot on bind,
//!   rebuilt on doc generation change).
//! - Rectangle + label visuals; straight-line edges.
//! - Drag-to-move nodes → mutates the local scene (feedback only —
//!   doc ops from drag land in B3).
//! - Pan / zoom / select / rubber-band / Delete / F-to-fit — all via
//!   the default `Canvas` wiring, nothing to wire here.
//!
//! Icon rendering (SVG via `usvg`), animated wires, widget-in-node
//! plots, and doc-op emission all land later as new visual impls /
//! in the projector's write-back path — no canvas changes required.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_canvas::{
    Canvas, DrawCtx, Edge as CanvasEdge, EdgeVisual, NavBarOverlay, Node as CanvasNode,
    NodeId as CanvasNodeId, NodeVisual, Pos as CanvasPos, Port as CanvasPort,
    PortId as CanvasPortId, PortRef, Rect as CanvasRect, Scene, VisualRegistry,
};
use lunco_workbench::{Panel, PanelId, PanelSlot};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

use crate::ui::state::{ModelicaDocumentRegistry, WorkbenchState};
use crate::visual_diagram::{DiagramNodeId, VisualDiagram};
// `Document` is the trait that exposes `.generation()` on
// `ModelicaDocument`; `DocumentHost::document()` returns a bare `&D`
// so we need the trait in scope to call generation on it.
use lunco_doc::Document;

pub const CANVAS_DIAGRAM_PANEL_ID: PanelId = PanelId("modelica_canvas_diagram");

// ─── Visuals ────────────────────────────────────────────────────────

/// Minimum viable icon visual — rounded rect, label, ports on the
/// boundary. No SVG yet; that lands once the canvas flow feels
/// right and we're ready to retire the snarl panel.
#[derive(Default)]
struct IconNodeVisual {
    /// Type name ("Resistor", "Capacitor"…) rendered as a hint
    /// under the instance label.
    type_label: String,
}

impl NodeVisual for IconNodeVisual {
    fn draw(&self, ctx: &mut DrawCtx, node: &CanvasNode, selected: bool) {
        let r = ctx
            .viewport
            .world_rect_to_screen(node.rect, ctx.screen_rect);
        let rect = egui::Rect::from_min_max(
            egui::pos2(r.min.x, r.min.y),
            egui::pos2(r.max.x, r.max.y),
        );
        let painter = ctx.ui.painter();
        let fill = egui::Color32::from_rgb(48, 56, 72);
        let stroke = if selected {
            egui::Stroke::new(2.0, egui::Color32::from_rgb(120, 170, 255))
        } else {
            egui::Stroke::new(1.5, egui::Color32::from_rgb(100, 110, 130))
        };
        painter.rect_filled(rect, 6.0, fill);
        painter.rect_stroke(rect, 6.0, stroke, egui::StrokeKind::Outside);
        // Instance name.
        if !node.label.is_empty() {
            painter.text(
                egui::pos2(rect.center().x, rect.min.y + 14.0),
                egui::Align2::CENTER_CENTER,
                &node.label,
                egui::FontId::proportional(12.0),
                egui::Color32::from_rgb(220, 225, 235),
            );
        }
        if !self.type_label.is_empty() && rect.height() > 30.0 {
            painter.text(
                egui::pos2(rect.center().x, rect.max.y - 10.0),
                egui::Align2::CENTER_CENTER,
                &self.type_label,
                egui::FontId::proportional(10.0),
                egui::Color32::from_rgb(150, 160, 175),
            );
        }
        // Ports as small filled circles.
        for port in &node.ports {
            let world = CanvasPos::new(
                node.rect.min.x + port.local_offset.x,
                node.rect.min.y + port.local_offset.y,
            );
            let p = ctx.viewport.world_to_screen(world, ctx.screen_rect);
            painter.circle_filled(
                egui::pos2(p.x, p.y),
                4.0,
                egui::Color32::from_rgb(200, 210, 230),
            );
            painter.circle_stroke(
                egui::pos2(p.x, p.y),
                4.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgb(40, 50, 70)),
            );
        }
    }
    fn debug_name(&self) -> &str {
        "modelica.icon"
    }
}

/// Straight-line edge visual. Upgrade path: a Bezier or orthogonal-
/// routed variant is another impl of `EdgeVisual` — register under a
/// different kind id.
struct StraightEdgeVisual;

impl EdgeVisual for StraightEdgeVisual {
    fn draw(
        &self,
        ctx: &mut DrawCtx,
        from: CanvasPos,
        to: CanvasPos,
        selected: bool,
    ) {
        let col = if selected {
            egui::Color32::from_rgb(140, 190, 255)
        } else {
            egui::Color32::from_rgb(180, 190, 210)
        };
        ctx.ui.painter().line_segment(
            [egui::pos2(from.x, from.y), egui::pos2(to.x, to.y)],
            egui::Stroke::new(if selected { 2.0 } else { 1.5 }, col),
        );
    }
}

fn build_registry() -> VisualRegistry {
    let mut reg = VisualRegistry::new();
    reg.register_node_kind("modelica.icon", |data: &JsonValue| {
        let type_label = data
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        IconNodeVisual { type_label }
    });
    reg.register_edge_kind("modelica.connection", |_: &JsonValue| StraightEdgeVisual);
    reg
}

// ─── Projection: VisualDiagram → lunco_canvas::Scene ────────────────

/// Modelica diagram coordinates are `(-100..100)` both axes with +Y
/// up, so we flip Y when projecting to the canvas (screen convention:
/// +Y down). Width is a fixed 80×60 world-unit box for MVP — the
/// real icon geometry comes from the Modelica `Icon` annotation; we
/// will read that in a follow-up.
const ICON_W: f32 = 80.0;
const ICON_H: f32 = 60.0;

fn project_scene(diagram: &VisualDiagram) -> (Scene, HashMap<DiagramNodeId, CanvasNodeId>) {
    let mut scene = Scene::new();
    let mut id_map: HashMap<DiagramNodeId, CanvasNodeId> = HashMap::new();

    for node in &diagram.nodes {
        let cid = scene.alloc_node_id();
        id_map.insert(node.id, cid);

        // Ports: map Modelica (-100..100, +Y up) to local box
        // (0..ICON_W, 0..ICON_H, +Y down).
        let ports: Vec<CanvasPort> = node
            .component_def
            .ports
            .iter()
            .map(|p| {
                // Modelica x,y are ∈ [-100, 100]; map to [0, W], [0, H].
                let lx = ((p.x + 100.0) / 200.0) * ICON_W;
                // Flip Y: Modelica +Y up, canvas +Y down.
                let ly = ((100.0 - p.y) / 200.0) * ICON_H;
                CanvasPort {
                    id: CanvasPortId::new(p.name.clone()),
                    local_offset: CanvasPos::new(lx, ly),
                    kind: p.connector_type.clone().into(),
                }
            })
            .collect();

        // Modelica Y-axis flip for node origin too — so the diagram
        // reads the same way as Dymola.
        let wx = node.position.x;
        let wy = -node.position.y; // flip Y

        scene.insert_node(CanvasNode {
            id: cid,
            rect: CanvasRect::from_min_size(
                CanvasPos::new(wx - ICON_W * 0.5, wy - ICON_H * 0.5),
                ICON_W,
                ICON_H,
            ),
            kind: "modelica.icon".into(),
            data: serde_json::json!({ "type": node.component_def.name }),
            ports,
            label: node.instance_name.clone(),
            origin: Some(node.instance_name.clone()),
        });
    }

    for edge in &diagram.edges {
        let Some(src_cid) = id_map.get(&edge.source_node) else {
            continue;
        };
        let Some(tgt_cid) = id_map.get(&edge.target_node) else {
            continue;
        };
        let eid = scene.alloc_edge_id();
        scene.insert_edge(CanvasEdge {
            id: eid,
            from: PortRef {
                node: *src_cid,
                port: CanvasPortId::new(edge.source_port.clone()),
            },
            to: PortRef {
                node: *tgt_cid,
                port: CanvasPortId::new(edge.target_port.clone()),
            },
            kind: "modelica.connection".into(),
            data: JsonValue::Null,
            origin: None,
        });
    }

    (scene, id_map)
}

// ─── Panel state + Bevy resource ───────────────────────────────────

/// Per-panel state carried across frames. Stored as a Bevy resource so
/// the panel's `render` can pull it out via `world.resource_mut`.
#[derive(Resource)]
pub struct CanvasDiagramState {
    pub canvas: Canvas,
    /// Last doc generation we projected — used to skip the project
    /// step when nothing has changed upstream.
    pub last_seen_gen: u64,
    /// Which doc the scene currently reflects; `None` when no doc is
    /// bound. Cleared on doc switch so the next frame's render does
    /// a full rebuild.
    pub bound_doc: Option<lunco_doc::DocumentId>,
}

impl Default for CanvasDiagramState {
    fn default() -> Self {
        let mut canvas = Canvas::new(build_registry());
        // Ship with a NavBar overlay so users have discoverable
        // zoom / fit controls. Miro/Figma convention.
        canvas.overlays.push(Box::new(NavBarOverlay::default()));
        Self {
            canvas,
            last_seen_gen: 0,
            bound_doc: None,
        }
    }
}

// ─── Panel ─────────────────────────────────────────────────────────

pub struct CanvasDiagramPanel;

impl Panel for CanvasDiagramPanel {
    fn id(&self) -> PanelId {
        CANVAS_DIAGRAM_PANEL_ID
    }
    fn title(&self) -> String {
        "🧩 Canvas Diagram".into()
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Ensure the state resource exists before we poke it.
        if world.get_resource::<CanvasDiagramState>().is_none() {
            world.insert_resource(CanvasDiagramState::default());
        }

        // Decide whether to rebuild the scene. We use the existing
        // snarl panel's `DiagramState.diagram` as the projection
        // source — both panels read from it, only the snarl one
        // currently writes to it. When canvas gains write-back,
        // this indirection can go away.
        let project_now = {
            let Some(open) = world.resource::<WorkbenchState>().open_model.clone() else {
                // No model open — render empty canvas and bail.
                world
                    .resource_mut::<CanvasDiagramState>()
                    .canvas
                    .scene = Scene::new();
                self.render_canvas(ui, world);
                return;
            };
            let Some(doc_id) = open.doc else {
                self.render_canvas(ui, world);
                return;
            };
            let gen = world
                .resource::<ModelicaDocumentRegistry>()
                .host(doc_id)
                .map(|h| h.document().generation())
                .unwrap_or(0);
            let state = world.resource::<CanvasDiagramState>();
            let bound_changed = state.bound_doc != Some(doc_id);
            let gen_advanced = gen != state.last_seen_gen;
            (bound_changed || gen_advanced).then_some((doc_id, gen))
        };

        if let Some((doc_id, gen)) = project_now {
            let diagram = world
                .resource::<crate::ui::panels::diagram::DiagramState>()
                .diagram
                .clone();
            let (scene, _id_map) = project_scene(&diagram);
            let mut state = world.resource_mut::<CanvasDiagramState>();
            let doc_switched = state.bound_doc != Some(doc_id);
            state.canvas.scene = scene;
            state.canvas.selection.clear();
            state.last_seen_gen = gen;
            state.bound_doc = Some(doc_id);
            // On first bind / doc switch, frame the scene so users
            // land with content centered instead of "why is it blank".
            // Subsequent gen bumps (user edit) keep the camera put so
            // their mental map isn't disturbed.
            if doc_switched {
                if let Some(world_rect) = state.canvas.scene.bounds() {
                    // Needs a sensible screen rect — we don't have
                    // one until render happens. Approximate with
                    // 800×600 and let the next few frames ease if
                    // needed; worst case user presses F to refit
                    // once the layout settles.
                    let screen = lunco_canvas::Rect::from_min_max(
                        lunco_canvas::Pos::new(0.0, 0.0),
                        lunco_canvas::Pos::new(800.0, 600.0),
                    );
                    let (c, z) = state
                        .canvas
                        .viewport
                        .fit_values(world_rect, screen, 40.0);
                    state.canvas.viewport.snap_to(c, z);
                }
            }
        }

        self.render_canvas(ui, world);
    }
}

impl CanvasDiagramPanel {
    fn render_canvas(&self, ui: &mut egui::Ui, world: &mut World) {
        let mut state = world.resource_mut::<CanvasDiagramState>();
        let _events = state.canvas.ui(ui);
        // B2: we don't yet translate scene events back to doc ops.
        // That lands in B3 — the Modelica projector will map
        // `NodeMoved` → `SetPlacement`, `EdgeCreated` → `AddConnection`,
        // etc., going through the existing command bus.
    }
}
