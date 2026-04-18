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
use crate::visual_diagram::{
    msl_categories, msl_components_in_category, DiagramNodeId, MSLComponentDef, VisualDiagram,
};
// `Document` is the trait that exposes `.generation()` on
// `ModelicaDocument`; `DocumentHost::document()` returns a bare `&D`
// so we need the trait in scope to call generation on it.
use lunco_doc::Document;
// Modelica op set + pretty-printer types for constructing payloads.
use crate::document::ModelicaOp;
use crate::pretty::{self, Placement};

pub const CANVAS_DIAGRAM_PANEL_ID: PanelId = PanelId("modelica_canvas_diagram");

// ─── Visuals ────────────────────────────────────────────────────────

/// Process-wide cache of SVG icon bytes keyed by relative asset
/// path. Loaded lazily on first request for a path; later requests
/// return the shared buffer. Entries live forever — icon asset
/// files don't change at runtime, and the total size is small.
fn svg_bytes_for(asset_path: &str) -> Option<std::sync::Arc<Vec<u8>>> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<std::collections::HashMap<String, Option<std::sync::Arc<Vec<u8>>>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut map = cache.lock().expect("svg cache poisoned");
    if let Some(cached) = map.get(asset_path) {
        return cached.clone();
    }
    // SVG assets ship inside the MSL cache dir (see snarl panel's
    // `draw_symbol_v2` for the reference resolution). Icon paths
    // come from `msl_index.json` and are relative to that dir.
    let full = lunco_assets::msl_dir().join(asset_path);
    let loaded = std::fs::read(&full).ok().map(std::sync::Arc::new);
    map.insert(asset_path.to_string(), loaded.clone());
    loaded
}

/// Per-component icon visual. Renders the Modelica SVG icon if the
/// component's `icon_asset` path resolved, else a stylised
/// rounded-rectangle fallback with the instance name. Ports render
/// as filled dots on the icon boundary.
#[derive(Default)]
struct IconNodeVisual {
    /// Type name ("Resistor", "Capacitor"…) shown under the instance
    /// label when no SVG is available.
    type_label: String,
    /// Relative asset path of the icon SVG, or empty when the
    /// component has no icon. Looked up in the shared cache each
    /// draw.
    icon_asset: String,
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

        // Try the SVG path first. If the asset loaded, paint it over
        // a subtle card so the icon has contrast with the diagram
        // background. If anything's missing, fall through to the
        // rounded-rect fallback so the user still sees SOMETHING.
        let mut drew_svg = false;
        if !self.icon_asset.is_empty() {
            if let Some(bytes) = svg_bytes_for(&self.icon_asset) {
                super::svg_renderer::draw_svg_to_egui(painter, rect, &bytes);
                drew_svg = true;
            }
        }

        if !drew_svg {
            // Fallback card + type label.
            let fill = egui::Color32::from_rgb(48, 56, 72);
            painter.rect_filled(rect, 6.0, fill);
            if !self.type_label.is_empty() && rect.height() > 30.0 {
                painter.text(
                    egui::pos2(rect.center().x, rect.center().y),
                    egui::Align2::CENTER_CENTER,
                    &self.type_label,
                    egui::FontId::proportional(10.0),
                    egui::Color32::from_rgb(200, 210, 225),
                );
            }
        }

        // Selection outline draws ON TOP of the icon so it's always
        // visible even over busy SVG content.
        let stroke = if selected {
            egui::Stroke::new(2.0, egui::Color32::from_rgb(120, 170, 255))
        } else {
            egui::Stroke::new(1.0, egui::Color32::from_rgb(90, 100, 120))
        };
        painter.rect_stroke(rect, 6.0, stroke, egui::StrokeKind::Outside);

        // Instance name above the icon.
        if !node.label.is_empty() {
            painter.text(
                egui::pos2(rect.center().x, rect.min.y - 4.0),
                egui::Align2::CENTER_BOTTOM,
                &node.label,
                egui::FontId::proportional(11.0),
                egui::Color32::from_rgb(220, 225, 235),
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

/// Dymola / OMEdit-style orthogonal edge: one horizontal-vertical-
/// horizontal Z-route with the bend at the x-midpoint. Collapses to
/// a straight segment when the endpoints are (near-)collinear on
/// either axis, avoiding degenerate zero-length jogs.
///
/// A richer routing pass (obstacle-avoidance, port-direction-aware
/// stubs, multiple-bend auto-layout) is a next step; this is the
/// pattern users already recognise.
struct OrthogonalEdgeVisual;

impl EdgeVisual for OrthogonalEdgeVisual {
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
            egui::Color32::from_rgb(60, 120, 200)
        };
        let width = if selected { 2.0 } else { 1.4 };
        let stroke = egui::Stroke::new(width, col);
        let painter = ctx.ui.painter();

        let dx = to.x - from.x;
        let dy = to.y - from.y;
        // Near-collinear: straight segment. Threshold in screen
        // pixels keeps it stable at all zoom levels (the caller
        // already transformed to screen-space).
        let thr = 1.0;
        if dx.abs() < thr || dy.abs() < thr {
            painter.line_segment(
                [egui::pos2(from.x, from.y), egui::pos2(to.x, to.y)],
                stroke,
            );
            return;
        }

        // Z-route with the bend at the x midpoint. Produces the
        // classic Modelica "two right-angle bends" shape:
        //
        //   A─────┐
        //         │
        //         └─────B
        let midx = from.x + dx * 0.5;
        let p0 = egui::pos2(from.x, from.y);
        let p1 = egui::pos2(midx, from.y);
        let p2 = egui::pos2(midx, to.y);
        let p3 = egui::pos2(to.x, to.y);
        painter.line_segment([p0, p1], stroke);
        painter.line_segment([p1, p2], stroke);
        painter.line_segment([p2, p3], stroke);
    }

    /// Hit-test each of the three segments individually so clicks
    /// near the bend register, not just clicks near the phantom
    /// straight line between endpoints.
    fn hit(
        &self,
        world_pos: lunco_canvas::Pos,
        from_world: lunco_canvas::Pos,
        to_world: lunco_canvas::Pos,
    ) -> bool {
        let threshold_sq = 16.0_f32;
        let dx = to_world.x - from_world.x;
        let dy = to_world.y - from_world.y;
        if dx.abs() < 1.0 || dy.abs() < 1.0 {
            return segment_dist_sq(world_pos, from_world, to_world) <= threshold_sq;
        }
        let midx = from_world.x + dx * 0.5;
        let p0 = from_world;
        let p1 = lunco_canvas::Pos::new(midx, from_world.y);
        let p2 = lunco_canvas::Pos::new(midx, to_world.y);
        let p3 = to_world;
        segment_dist_sq(world_pos, p0, p1) <= threshold_sq
            || segment_dist_sq(world_pos, p1, p2) <= threshold_sq
            || segment_dist_sq(world_pos, p2, p3) <= threshold_sq
    }
}

/// Squared perpendicular distance from `p` to the finite segment
/// `(a,b)`. Endpoint-clamped — clicking past the end doesn't count.
fn segment_dist_sq(
    p: lunco_canvas::Pos,
    a: lunco_canvas::Pos,
    b: lunco_canvas::Pos,
) -> f32 {
    let ax = b.x - a.x;
    let ay = b.y - a.y;
    let len_sq = ax * ax + ay * ay;
    if len_sq < f32::EPSILON {
        let dx = p.x - a.x;
        let dy = p.y - a.y;
        return dx * dx + dy * dy;
    }
    let t = (((p.x - a.x) * ax + (p.y - a.y) * ay) / len_sq).clamp(0.0, 1.0);
    let foot_x = a.x + t * ax;
    let foot_y = a.y + t * ay;
    let dx = p.x - foot_x;
    let dy = p.y - foot_y;
    dx * dx + dy * dy
}

fn build_registry() -> VisualRegistry {
    let mut reg = VisualRegistry::new();
    reg.register_node_kind("modelica.icon", |data: &JsonValue| {
        let type_label = data
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let icon_asset = data
            .get("icon_asset")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        IconNodeVisual {
            type_label,
            icon_asset,
        }
    });
    reg.register_edge_kind("modelica.connection", |_: &JsonValue| OrthogonalEdgeVisual);
    reg
}

// ─── Projection: VisualDiagram → lunco_canvas::Scene ────────────────

/// Modelica diagram coordinates are `(-100..100)` both axes with +Y
/// up. Width is a fixed 20×20 world-unit box — the typical
/// Modelica icon extent (`{{-10,-10},{10,10}}`). Dymola/OMEdit
/// render components at this size by default. Reading the actual
/// per-component `Icon` annotation extent is a follow-up.
const ICON_W: f32 = 20.0;
const ICON_H: f32 = 20.0;

/// Coordinate-system types + the two conversion functions between
/// them. Named wrappers around plain `(f32, f32)` so every place
/// the sign flip happens is explicit and typed — previously we had
/// ad-hoc `-y` negations scattered across the projector, the op
/// emitters, and the context-menu handler, and a missing negation
/// or a double-negation produced the hard-to-diagnose "position is
/// off" class of bugs.
///
/// Conventions:
///
/// - [`ModelicaPos`] — Modelica `.mo` source convention. +Y up.
///   Ranges typically `-100..100` per axis. This is the authored
///   coordinate that lands in `annotation(Placement(...))`.
///
/// - [`lunco_canvas::Pos`] — canvas world coordinate. +Y DOWN
///   (screen convention). This is what the canvas scene / viewport
///   consume and what hit-testing / rendering operates on.
///
/// The two differ only in the sign of Y. Keeping them as separate
/// types makes mis-conversion a type error instead of a silent off-
/// by-flip.
pub mod coords {
    use lunco_canvas::Pos as CanvasPos;

    /// Modelica-convention 2D point (+Y up).
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct ModelicaPos {
        pub x: f32,
        pub y: f32,
    }

    impl ModelicaPos {
        pub const fn new(x: f32, y: f32) -> Self {
            Self { x, y }
        }
    }

    /// Canvas world (+Y down) → Modelica (+Y up).
    #[inline]
    pub fn canvas_to_modelica(c: CanvasPos) -> ModelicaPos {
        ModelicaPos {
            x: c.x,
            y: -c.y,
        }
    }

    /// Modelica (+Y up) → canvas world (+Y down).
    #[inline]
    pub fn modelica_to_canvas(m: ModelicaPos) -> CanvasPos {
        CanvasPos::new(m.x, -m.y)
    }

    /// Canvas rect-min → Modelica centre. Used when committing a
    /// drag: the user's drag target lands as the icon's top-left in
    /// canvas coordinates, but Modelica placements are centre-
    /// anchored, so we shift by half the icon extent.
    #[inline]
    pub fn canvas_min_to_modelica_center(
        min: CanvasPos,
        icon_w: f32,
        icon_h: f32,
    ) -> ModelicaPos {
        canvas_to_modelica(CanvasPos::new(
            min.x + icon_w * 0.5,
            min.y + icon_h * 0.5,
        ))
    }
}

use coords::{canvas_to_modelica, ModelicaPos};

/// Fallback port layout when the component has no annotated port
/// positions. Alternates left / right edges at the vertical centre
/// for the first two ports (the common two-terminal shape), then
/// walks up both sides for any additional ports.
fn port_fallback_offset(index: usize, _total: usize) -> (f32, f32) {
    let side_left = index % 2 == 0;
    let row = index / 2; // 0 → middle, 1 → above, 2 → even higher
    let cy = ICON_H * 0.5 - (row as f32) * (ICON_H * 0.25);
    let cx = if side_left { 0.0 } else { ICON_W };
    (cx, cy.clamp(0.0, ICON_H))
}

/// Regex-scan `connect(a.b, c.d);` patterns in `source` and add
/// matching edges to `diagram`. Skips equations whose components
/// aren't in the diagram (missing nodes stay visually missing) or
/// that already exist as edges (keyed by unordered endpoint pair).
///
/// Deliberately permissive: doesn't validate port existence, doesn't
/// care about the line-continuation form, doesn't consult
/// annotations. "Text says A.x ↔ B.y; show a line between A and B".
fn recover_edges_from_source(source: &str, diagram: &mut VisualDiagram) {
    // (?m) lets `.` not cross newlines by default; we explicitly
    // allow whitespace/newline runs via `\s*`. Capture groups:
    //   1 = src component, 2 = src port
    //   3 = tgt component, 4 = tgt port
    // Port names allow `.` so we catch qualified sub-ports
    // (`flange.phi`), though most cases are one dot deep.
    static CONNECT_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = CONNECT_RE.get_or_init(|| {
        regex::Regex::new(
            r"connect\s*\(\s*([A-Za-z_]\w*)\s*\.\s*([A-Za-z_][\w\.]*)\s*,\s*([A-Za-z_]\w*)\s*\.\s*([A-Za-z_][\w\.]*)\s*\)",
        )
        .expect("connect regex compiles")
    });

    // Build (instance_name → DiagramNodeId) index once per call.
    // Own the keys so we can freely mutate `diagram` below.
    let index: HashMap<String, DiagramNodeId> = diagram
        .nodes
        .iter()
        .map(|n| (n.instance_name.clone(), n.id))
        .collect();

    // Track existing edges as unordered pairs so we don't double-
    // add when the AST path already caught a connection.
    let existing: std::collections::HashSet<((String, String), (String, String))> = diagram
        .edges
        .iter()
        .map(|e| {
            let a = (
                diagram
                    .get_node(e.source_node)
                    .map(|n| n.instance_name.clone())
                    .unwrap_or_default(),
                e.source_port.clone(),
            );
            let b = (
                diagram
                    .get_node(e.target_node)
                    .map(|n| n.instance_name.clone())
                    .unwrap_or_default(),
                e.target_port.clone(),
            );
            // Canonicalise to min/max so (A.x, B.y) == (B.y, A.x).
            if a <= b { (a, b) } else { (b, a) }
        })
        .collect();

    for cap in re.captures_iter(source) {
        let src_comp = &cap[1];
        let src_port = cap[2].to_string();
        let tgt_comp = &cap[3];
        let tgt_port = cap[4].to_string();

        let (Some(&src_id), Some(&tgt_id)) =
            (index.get(src_comp), index.get(tgt_comp))
        else {
            continue;
        };

        let pair = {
            let a = (src_comp.to_string(), src_port.clone());
            let b = (tgt_comp.to_string(), tgt_port.clone());
            if a <= b { (a, b) } else { (b, a) }
        };
        if existing.contains(&pair) {
            continue;
        }

        diagram.add_edge(src_id, src_port, tgt_id, tgt_port);
    }
}

fn project_scene(diagram: &VisualDiagram) -> (Scene, HashMap<DiagramNodeId, CanvasNodeId>) {
    let mut scene = Scene::new();
    let mut id_map: HashMap<DiagramNodeId, CanvasNodeId> = HashMap::new();

    for node in &diagram.nodes {
        let cid = scene.alloc_node_id();
        id_map.insert(node.id, cid);

        // Ports: map Modelica (-100..100, +Y up) to local icon box
        // (0..ICON_W, 0..ICON_H, +Y down). If a port has no
        // annotated position (both x and y at 0 — the default when
        // the component class didn't declare one), fall back to
        // distributing around the icon's edges: alternating left
        // and right for the classic two-terminal electrical shape,
        // extending up for more ports. Matches what OMEdit does
        // when Placement annotations are missing.
        let n_ports = node.component_def.ports.len();
        let ports: Vec<CanvasPort> = node
            .component_def
            .ports
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let (lx, ly) = if p.x == 0.0 && p.y == 0.0 {
                    port_fallback_offset(i, n_ports)
                } else {
                    let lx = ((p.x + 100.0) / 200.0) * ICON_W;
                    let ly = ((100.0 - p.y) / 200.0) * ICON_H;
                    (lx, ly)
                };
                CanvasPort {
                    id: CanvasPortId::new(p.name.clone()),
                    local_offset: CanvasPos::new(lx, ly),
                    kind: p.connector_type.clone().into(),
                }
            })
            .collect();

        // `DiagramNode.position` is already stored in screen / +Y-down
        // convention — `import_model_to_diagram` flips the Modelica
        // annotation's +Y-up coordinate at read time (see diagram.rs
        // around the Placement-annotation regex, which does
        // `y = -((y1+y2)/2.0)`). We therefore use the stored Y
        // directly here; flipping it again (as an earlier version
        // of this function did) places components at the wrong
        // side of the diagram and makes right-click "add component"
        // appear to ignore zoom / offset.
        let wx = node.position.x;
        let wy = node.position.y;

        scene.insert_node(CanvasNode {
            id: cid,
            rect: CanvasRect::from_min_size(
                CanvasPos::new(wx - ICON_W * 0.5, wy - ICON_H * 0.5),
                ICON_W,
                ICON_H,
            ),
            kind: "modelica.icon".into(),
            data: serde_json::json!({
                "type": node.component_def.name,
                "icon_asset": node.component_def.icon_asset.clone().unwrap_or_default(),
            }),
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
    /// Pending context-menu request from the canvas. Populated when
    /// the canvas emits `ContextMenuRequested`; rendered as an egui
    /// popup on the next frame and cleared when the user clicks
    /// outside or picks an entry.
    pub context_menu: Option<PendingContextMenu>,
}

/// Snapshot of a right-click: where to anchor the popup + what it
/// was targeted at. Close handling is done via egui's
/// `clicked_elsewhere()` on the popup's Response — no manual timer.
#[derive(Debug, Clone)]
pub struct PendingContextMenu {
    pub screen_pos: egui::Pos2,
    /// World position at click time — used when an "Add component"
    /// entry is selected so the new component lands where the user
    /// right-clicked, not at (0,0).
    pub world_pos: lunco_canvas::Pos,
    pub target: ContextMenuTarget,
}

#[derive(Debug, Clone)]
pub enum ContextMenuTarget {
    Node(lunco_canvas::NodeId),
    Edge(lunco_canvas::EdgeId),
    Empty,
}

impl Default for CanvasDiagramState {
    fn default() -> Self {
        let mut canvas = Canvas::new(build_registry());
        // Drop the default `SelectionLayer` — IconNodeVisual draws
        // its own selection halo at the exact same corner radius
        // as the icon body, so a separate selection layer would
        // show a mismatched outline. Removing it here keeps the
        // canvas crate's default pipeline generic while the
        // Modelica panel opts out.
        canvas.layers.retain(|layer| layer.name() != "selection");
        canvas.overlays.push(Box::new(NavBarOverlay::default()));
        Self {
            canvas,
            last_seen_gen: 0,
            bound_doc: None,
            context_menu: None,
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
            // Read the source directly from the document host and
            // parse it into a VisualDiagram ourselves — do NOT rely
            // on the snarl panel's `DiagramState.diagram` being
            // populated, which only happens if that panel was
            // rendered first.
            let source = world
                .resource::<ModelicaDocumentRegistry>()
                .host(doc_id)
                .map(|h| h.document().source().to_string())
                .unwrap_or_default();
            let mut diagram = crate::ui::panels::diagram::import_model_to_diagram(&source)
                .unwrap_or_default();
            // AST-based edge construction inside `import_model_to_diagram`
            // drops connect equations whose port names don't resolve
            // through its component-graph port index — common when
            // the MSL palette entry carries synthetic / partial
            // port names that don't exactly match the Modelica
            // class's actual connectors. Source text is authoritative,
            // so do a regex recovery pass: any `connect(a.b, c.d)`
            // with both components present in the diagram becomes
            // an edge. Gives us "text says it, diagram shows it"
            // without touching upstream parser behaviour.
            recover_edges_from_source(&source, &mut diagram);
            bevy::log::info!(
                "[CanvasDiagram] project: {} nodes, {} edges from diagram",
                diagram.nodes.len(),
                diagram.edges.len(),
            );
            let (scene, _id_map) = project_scene(&diagram);
            bevy::log::info!(
                "[CanvasDiagram] project: {} scene nodes, {} scene edges after projection",
                scene.node_count(),
                scene.edge_count(),
            );
            let mut state = world.resource_mut::<CanvasDiagramState>();
            let doc_switched = state.bound_doc != Some(doc_id);
            state.canvas.scene = scene;
            state.canvas.selection.clear();
            state.last_seen_gen = gen;
            state.bound_doc = Some(doc_id);
            // On first bind / doc switch, frame the scene so users
            // land with content centered. If the scene is empty
            // (fresh model), fall back to physical zoom so the
            // first component the user adds lands at a legible size
            // — world units are Modelica millimetres, `physical_mm_zoom`
            // maps them 1 : 1 to the screen using the DPI reported
            // by the egui context.
            if doc_switched {
                let physical_zoom =
                    lunco_canvas::Viewport::physical_mm_zoom(ui.ctx());
                if let Some(world_rect) = state.canvas.scene.bounds() {
                    let screen = lunco_canvas::Rect::from_min_max(
                        lunco_canvas::Pos::new(0.0, 0.0),
                        lunco_canvas::Pos::new(800.0, 600.0),
                    );
                    let (c, z) = state
                        .canvas
                        .viewport
                        .fit_values(world_rect, screen, 40.0);
                    // Clamp the fit zoom to at least physical, so a
                    // small scene doesn't zoom absurdly far in.
                    let z = z.min(physical_zoom * 2.0).max(physical_zoom * 0.5);
                    state.canvas.viewport.snap_to(c, z);
                } else {
                    // Empty scene — land at physical mm zoom,
                    // centred on the Modelica origin.
                    state.canvas.viewport.snap_to(
                        lunco_canvas::Pos::new(0.0, 0.0),
                        physical_zoom,
                    );
                }
            }
        }

        self.render_canvas(ui, world);
    }
}

impl CanvasDiagramPanel {
    fn render_canvas(&self, ui: &mut egui::Ui, world: &mut World) {
        // Resolve editing class + doc id up front. These drive op
        // emission; without them (no doc bound, or unparseable
        // source) the canvas stays read-only — events still fire
        // but translate to nothing, matching "no-op on empty doc".
        let (doc_id, editing_class) = resolve_doc_context(world);

        // Render the canvas and collect its events.
        let (response, events) = {
            let mut state = world.resource_mut::<CanvasDiagramState>();
            state.canvas.ui(ui)
        };

        // Empty-diagram overlay: when the class has no components
        // (equation-only models like Battery, SpringMass,
        // RocketEngine), show a summary card instead of an
        // uninformative empty grid. OMEdit's diagram view is empty
        // for these too, but we can do better by giving users a
        // direct cue about what's in the model and where to find it.
        let show_empty_overlay = {
            let state = world.resource::<CanvasDiagramState>();
            state.canvas.scene.node_count() == 0
        };
        if show_empty_overlay {
            render_empty_diagram_overlay(ui, response.rect, world);
        }

        // Capture the right-click world position the frame the menu
        // opens — before egui's `press_origin` gets overwritten by
        // later clicks (on menu entries themselves, which would
        // otherwise become the hit-test origin and make a click on
        // empty space appear to have hit a node, or place a newly
        // added component under the menu instead of under the click).
        //
        // The cached value lives on `CanvasDiagramState.context_menu`
        // and is consumed when the menu closes.
        let screen_rect = lunco_canvas::Rect::from_min_max(
            lunco_canvas::Pos::new(response.rect.min.x, response.rect.min.y),
            lunco_canvas::Pos::new(response.rect.max.x, response.rect.max.y),
        );
        // Read whether egui's popup is currently open BEFORE any of
        // our logic runs. This is our ground truth for "is a menu
        // showing right now" — more reliable than our own cache
        // sync, because `context_menu` may open/close between frames
        // without going through our code path.
        let popup_was_open_before = ui.ctx().memory(|m| m.any_popup_open());

        // Track whether this frame wants to dismiss (second-right-
        // click to close). If so, we SKIP `response.context_menu()`
        // entirely for this frame so egui doesn't re-open on the
        // same secondary_clicked signal.
        let mut suppress_menu = false;

        if response.secondary_clicked() {
            let press = ui.ctx().input(|i| i.pointer.press_origin());
            if let Some(p) = press.or_else(|| response.interact_pointer_pos()) {
                if popup_was_open_before {
                    // Second right-click while the menu is up → dismiss.
                    // We BOTH clear our cache AND ask egui to close
                    // any popup. Skipping `context_menu` below prevents
                    // egui from re-opening on this same frame.
                    world.resource_mut::<CanvasDiagramState>().context_menu = None;
                    ui.ctx().memory_mut(|m| m.close_all_popups());
                    suppress_menu = true;
                } else {
                    // Fresh right-click: capture world position +
                    // hit-test origin while `press_origin` still
                    // reflects the right-click (before any menu-entry
                    // click overwrites it).
                    let state = world.resource::<CanvasDiagramState>();
                    let world_pos = state.canvas.viewport.screen_to_world(
                        lunco_canvas::Pos::new(p.x, p.y),
                        screen_rect,
                    );
                    let hit_node = state.canvas.scene.hit_node(world_pos, 6.0);
                    let hit_edge = state.canvas.scene.hit_edge(world_pos, 4.0);
                    let target = match (hit_node, hit_edge) {
                        (Some((id, _)), _) => ContextMenuTarget::Node(id),
                        (_, Some(id)) => ContextMenuTarget::Edge(id),
                        _ => ContextMenuTarget::Empty,
                    };
                    drop(state);
                    bevy::log::info!(
                        "[CanvasDiagram] right-click screen=({:.1},{:.1}) world=({:.1},{:.1}) target={:?}",
                        p.x, p.y, world_pos.x, world_pos.y, target
                    );
                    world.resource_mut::<CanvasDiagramState>().context_menu =
                        Some(PendingContextMenu {
                            screen_pos: p,
                            world_pos,
                            target,
                        });
                }
            }
        }

        // ── Render menu via egui's native `context_menu`. ──
        // Content comes from the cached PendingContextMenu (above).
        // Skipped on the dismiss-frame so egui doesn't re-open.
        let menu_ops: Vec<ModelicaOp> = if suppress_menu {
            Vec::new()
        } else {
            let mut collected: Vec<ModelicaOp> = Vec::new();
            let cached = world
                .resource::<CanvasDiagramState>()
                .context_menu
                .clone();
            response.context_menu(|ui| {
                let Some(menu) = cached.as_ref() else {
                    // No cached data — shouldn't happen since
                    // context_menu only opens after secondary_clicked,
                    // but render a minimal placeholder just in case.
                    ui.label("(no click target)");
                    return;
                };
                match &menu.target {
                    ContextMenuTarget::Node(id) => {
                        render_node_menu(
                            ui,
                            world,
                            *id,
                            editing_class.as_deref(),
                            &mut collected,
                        );
                    }
                    ContextMenuTarget::Edge(id) => {
                        render_edge_menu(
                            ui,
                            world,
                            *id,
                            editing_class.as_deref(),
                            &mut collected,
                        );
                    }
                    ContextMenuTarget::Empty => {
                        render_empty_menu(
                            ui,
                            world,
                            menu.world_pos,
                            editing_class.as_deref(),
                            &mut collected,
                        );
                    }
                }
            });
            collected
        };

        // Sync our cache with egui's popup state, AFTER context_menu
        // has had a chance to open/close this frame. If egui closed
        // the popup (user clicked outside, pressed escape, picked
        // an entry) and we still have a cache, drop the cache.
        // Running this *after* keeps us from clearing the cache we
        // just populated on a fresh right-click.
        let popup_open_now = ui.ctx().memory(|m| m.any_popup_open());
        if !popup_open_now
            && world
                .resource::<CanvasDiagramState>()
                .context_menu
                .is_some()
        {
            world.resource_mut::<CanvasDiagramState>().context_menu = None;
        }

        // Double-click on a node → "drill in". Open the class that
        // the component instantiates as a new model view tab,
        // alongside the current one. Matches Dymola / OMEdit's
        // "go into this component" gesture.
        for ev in &events {
            if let lunco_canvas::SceneEvent::NodeDoubleClicked { id } = ev {
                let type_name = {
                    let state = world.resource::<CanvasDiagramState>();
                    state
                        .canvas
                        .scene
                        .node(*id)
                        .and_then(|n| n.data.get("type").and_then(|v| v.as_str()).map(str::to_string))
                };
                if let Some(qualified) = type_name {
                    drill_into_class(world, &qualified);
                }
            }
        }

        // Translate scene events → ModelicaOps and apply.
        if let (Some(doc_id), Some(class)) = (doc_id, editing_class.as_ref()) {
            let mut all_ops = build_ops_from_events(world, &events, class);
            all_ops.extend(menu_ops);
            if !all_ops.is_empty() {
                apply_ops(world, doc_id, all_ops);
            }
        } else if !menu_ops.is_empty() {
            bevy::log::warn!(
                "[CanvasDiagram] menu emitted {} op(s) but no editing class — discarded",
                menu_ops.len()
            );
        }
        // `events` is consumed by `build_ops_from_events`; suppress
        // the unused warning when `doc_id`/`class` were absent.
        let _ = events;

    }
}

// ─── Context-menu renderers ────────────────────────────────────────

fn render_node_menu(
    ui: &mut egui::Ui,
    world: &mut World,
    id: lunco_canvas::NodeId,
    editing_class: Option<&str>,
    out: &mut Vec<ModelicaOp>,
) {
    let (instance, type_name) = component_headers(world, id);
    ui.label(egui::RichText::new(&instance).strong());
    if !type_name.is_empty() {
        ui.label(egui::RichText::new(&type_name).weak().small());
    }
    ui.separator();
    if ui.button("✂ Delete").clicked() {
        if let Some(class) = editing_class {
            if let Some(op) = op_remove_component(world, id, class) {
                out.push(op);
            }
        }
        ui.close();
    }
    if ui.button("📋 Duplicate").clicked() {
        ui.close();
    }
    ui.separator();
    if ui.button("↧ Open class").clicked() {
        ui.close();
    }
    if ui.button("🔧 Parameters…").clicked() {
        ui.close();
    }
}

fn render_edge_menu(
    ui: &mut egui::Ui,
    world: &mut World,
    id: lunco_canvas::EdgeId,
    editing_class: Option<&str>,
    out: &mut Vec<ModelicaOp>,
) {
    ui.label(egui::RichText::new("Connection").strong());
    ui.separator();
    if ui.button("✂ Delete").clicked() {
        if let Some(class) = editing_class {
            if let Some(op) = op_remove_edge(world, id, class) {
                out.push(op);
            }
        }
        ui.close();
    }
    if ui.button("↺ Reverse direction").clicked() {
        ui.close();
    }
}

fn render_empty_menu(
    ui: &mut egui::Ui,
    world: &mut World,
    click_world: lunco_canvas::Pos,
    editing_class: Option<&str>,
    out: &mut Vec<ModelicaOp>,
) {
    ui.label(egui::RichText::new("Add component").strong());
    ui.separator();
    for cat in msl_categories() {
        let short = cat.split('/').last().unwrap_or(&cat).to_string();
        ui.menu_button(short, |ui| {
            for comp in msl_components_in_category(&cat) {
                if ui
                    .button(format!("{} {}", comp.display_name, comp.name))
                    .clicked()
                {
                    if let Some(class) = editing_class {
                        out.push(op_add_component(&comp, click_world, class));
                    }
                    ui.close();
                }
            }
        });
    }
    ui.separator();
    ui.label(egui::RichText::new("Common").weak().small());
    for quick_name in ["Resistor", "Capacitor", "Ground", "ConstantVoltage", "Inductor"] {
        if let Some(comp) = crate::visual_diagram::msl_component_library()
            .iter()
            .find(|c| c.name == quick_name)
        {
            if ui.button(quick_name).clicked() {
                if let Some(class) = editing_class {
                    out.push(op_add_component(comp, click_world, class));
                }
                ui.close();
            }
        }
    }
    ui.separator();
    if ui.button("⎚ Fit all (F)").clicked() {
        let mut state = world.resource_mut::<CanvasDiagramState>();
        if let Some(bounds) = state.canvas.scene.bounds() {
            let sr = lunco_canvas::Rect::from_min_max(
                lunco_canvas::Pos::new(0.0, 0.0),
                lunco_canvas::Pos::new(800.0, 600.0),
            );
            let (c, z) = state.canvas.viewport.fit_values(bounds, sr, 40.0);
            state.canvas.viewport.set_target(c, z);
        }
        ui.close();
    }
    if ui.button("⟲ Reset zoom").clicked() {
        let mut state = world.resource_mut::<CanvasDiagramState>();
        let c = state.canvas.viewport.center;
        state.canvas.viewport.set_target(c, 1.0);
        ui.close();
    }
}

// ─── Empty-diagram summary ──────────────────────────────────────────

/// When the canvas scene has no nodes (i.e. the class has no
/// component instantiations — common for equation-only models like
/// RocketEngine), paint a card in the centre of the canvas with a
/// summary of what the class *does* contain: parameters, inputs,
/// variables, equations. Tells the user the model is real and
/// points them at the Text tab, instead of leaving them staring
/// at a blank grid.
///
/// Summary numbers come from regex scans of the document source —
/// cheap, and the cost is only paid on frames where the scene is
/// empty (rare once a user opens a composite model).
fn render_empty_diagram_overlay(
    ui: &mut egui::Ui,
    canvas_rect: egui::Rect,
    world: &mut World,
) {
    let Some(open) = world.resource::<WorkbenchState>().open_model.as_ref() else {
        return;
    };
    let source = open.source.as_ref();
    let class_name = open.detected_name.clone().unwrap_or_else(|| "(unnamed)".into());

    let param_count = count_matches(source, r"(?m)^\s*parameter\s+");
    let input_count = count_matches(source, r"(?m)^\s*input\s+");
    let output_count = count_matches(source, r"(?m)^\s*output\s+");
    let equation_count = count_matches(
        source,
        r"(?m)^\s*(?:der\s*\(|[A-Za-z_]\w*\s*=\s*[^=])",
    );
    let connect_count = count_matches(source, r"\bconnect\s*\(");

    let card_w = 380.0;
    let card_h = 190.0;
    let card_rect = egui::Rect::from_center_size(
        canvas_rect.center(),
        egui::vec2(card_w, card_h),
    );

    let painter = ui.painter();
    // Card: rounded + slight drop shadow.
    painter.rect_filled(
        card_rect.translate(egui::vec2(0.0, 3.0)),
        10.0,
        egui::Color32::from_rgba_premultiplied(0, 0, 0, 100),
    );
    painter.rect_filled(
        card_rect,
        10.0,
        egui::Color32::from_rgb(34, 38, 48),
    );
    painter.rect_stroke(
        card_rect,
        10.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 70, 88)),
        egui::StrokeKind::Outside,
    );

    // Content via child UI so we get widget layout for free.
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(card_rect.shrink(16.0))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.label(
        egui::RichText::new("📝 Equation-only model")
            .strong()
            .size(14.0)
            .color(egui::Color32::from_rgb(220, 225, 235)),
    );
    child.label(
        egui::RichText::new(&class_name)
            .size(12.0)
            .color(egui::Color32::from_rgb(170, 185, 210)),
    );
    child.add_space(6.0);
    child.label(
        egui::RichText::new(
            "No instantiated components to draw. This class is defined \
             by equations — the composition view is empty by convention.",
        )
        .size(11.0)
        .color(egui::Color32::from_rgb(170, 180, 200)),
    );
    child.add_space(8.0);
    child.separator();
    child.add_space(6.0);

    let row = |u: &mut egui::Ui, label: &str, n: usize| {
        u.horizontal(|u| {
            u.label(
                egui::RichText::new(label)
                    .small()
                    .color(egui::Color32::from_rgb(150, 160, 180)),
            );
            u.with_layout(egui::Layout::right_to_left(egui::Align::Center), |u| {
                u.monospace(egui::RichText::new(format!("{n}")).color(
                    egui::Color32::from_rgb(200, 220, 255),
                ));
            });
        });
    };
    row(&mut child, "Parameters", param_count);
    row(&mut child, "Inputs", input_count);
    row(&mut child, "Outputs", output_count);
    row(&mut child, "Equations", equation_count);
    if connect_count > 0 {
        row(&mut child, "Connect equations", connect_count);
    }
    child.add_space(4.0);
    child.label(
        egui::RichText::new("→ Open the Text tab to read / edit the source.")
            .italics()
            .size(10.0)
            .color(egui::Color32::from_rgb(140, 155, 175)),
    );
}

/// Count regex matches in `source`. Each regex is compiled once
/// per call — cheap because this runs only on empty-scene frames.
fn count_matches(source: &str, pattern: &str) -> usize {
    regex::Regex::new(pattern)
        .ok()
        .map(|re| re.find_iter(source).count())
        .unwrap_or(0)
}

// ─── Drill-in ───────────────────────────────────────────────────────

/// Open the Modelica class with `qualified` name in a new model-view
/// tab alongside the current one. Mirrors Dymola / OMEdit's
/// "double-click to go into component" behaviour.
///
/// Resolution strategy (best-effort; MSL files are organised as
/// packages so a class might live inside any parent-level `.mo`):
///
/// 1. Try the per-class file `Modelica/…/{Short}.mo`.
/// 2. Walk parents — try `Modelica/…/Parent.mo` and check whether
///    `model Short` / `class Short` / `block Short` is declared
///    inside. Returns the matching file's full source (the whole
///    package file — the user may navigate among sibling classes).
///
/// If nothing resolves, logs a warning and no tab is opened. A
/// richer "Could not find class" toast is a follow-up.
fn drill_into_class(world: &mut World, qualified: &str) {
    // Only MSL classes supported in this first cut. User-defined
    // classes live in their own `.mo` files which the user has
    // presumably opened separately via the Package Browser.
    if !qualified.starts_with("Modelica.") {
        bevy::log::info!(
            "[CanvasDiagram] drill-in skipped — `{}` is not an MSL class (user classes TBD)",
            qualified
        );
        return;
    }
    let Some((source, file_path)) = resolve_msl_class_source(qualified) else {
        bevy::log::warn!(
            "[CanvasDiagram] drill-in: could not find MSL file containing `{}`",
            qualified
        );
        return;
    };
    open_readonly_tab(world, qualified, &source, &file_path);
}

/// Resolve a fully-qualified MSL class name to its source file via
/// the Modelica Specification's package layout rules — NOT by
/// guessing with regex across sibling files.
///
/// MSL (and every conforming Modelica library) uses one of two
/// on-disk layouts per class, per MLS §13.3:
///
/// 1. **Own-file class** — `A/B/C/Name.mo` containing `model Name
///    ... end Name;` at file scope. The common case for MSL leaf
///    classes.
/// 2. **Package-aggregated class** — `A/B/C/Name/package.mo`
///    declaring a `within A.B.C;` followed by `package Name
///    ... end Name;`. Used when the class itself hosts child
///    packages (e.g. `Modelica.Electrical.Analog`).
///
/// Given `A.B.C.Name`, this function tries those two paths in
/// order and returns the first that exists. No regex — the layout
/// is deterministic.
///
/// Returns `None` if neither path exists, which means either the
/// library snapshot is incomplete or the class lives outside the
/// MSL snapshot (future: also try user-project libraries). A
/// proper resolver would consult `MODELICAPATH` + any loaded
/// `package.mo`'s `within` declarations, but for MSL-only drill-in
/// the two-case layout check is sufficient and exact.
fn resolve_msl_class_source(qualified: &str) -> Option<(String, std::path::PathBuf)> {
    let msl_root = lunco_assets::msl_dir();
    let segments: Vec<&str> = qualified.split('.').collect();

    // Build the directory `A/B/C/Name/`, then try:
    //   A/B/C/Name.mo            (own-file class)
    //   A/B/C/Name/package.mo    (package-aggregated class)
    let mut base = msl_root.clone();
    for seg in &segments {
        base.push(seg);
    }

    let own_file = base.with_extension("mo");
    if let Ok(source) = std::fs::read_to_string(&own_file) {
        return Some((source, own_file));
    }

    let package_file = base.join("package.mo");
    if let Ok(source) = std::fs::read_to_string(&package_file) {
        return Some((source, package_file));
    }

    None
}

/// Allocate a read-only document for `qualified` class and open a
/// model-view tab. If a tab for the same document is already open,
/// the workbench focuses it instead of duplicating.
fn open_readonly_tab(
    world: &mut World,
    qualified: &str,
    source: &str,
    file_path: &std::path::Path,
) {
    // Skip if we already have an open tab on this exact path.
    let model_path_id = format!("msl://{qualified}");
    let existing_doc = world
        .resource::<WorkbenchState>()
        .open_model
        .as_ref()
        .filter(|m| m.model_path == model_path_id)
        .and_then(|m| m.doc);
    if let Some(doc_id) = existing_doc {
        // Already open — just focus.
        if let Some(mut layout) = world
            .get_resource_mut::<lunco_workbench::WorkbenchLayout>()
        {
            layout.open_instance(
                crate::ui::panels::model_view::MODEL_VIEW_KIND,
                doc_id.raw(),
            );
        }
        return;
    }

    let origin = lunco_doc::DocumentOrigin::File {
        path: file_path.to_path_buf(),
        writable: false,
    };
    let doc_id = {
        let mut registry = world.resource_mut::<ModelicaDocumentRegistry>();
        registry.allocate_with_origin(source.to_string(), origin)
    };

    // Update WorkbenchState.open_model so text/canvas panels bind
    // to the new doc. Same pattern the Package Browser uses.
    let source_arc: std::sync::Arc<str> = source.into();
    let mut line_starts = vec![0usize];
    for (i, b) in source.as_bytes().iter().enumerate() {
        if *b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    {
        let short = qualified.rsplit('.').next().unwrap_or(qualified).to_string();
        let mut ws = world.resource_mut::<WorkbenchState>();
        ws.open_model = Some(crate::ui::OpenModel {
            model_path: model_path_id.clone(),
            display_name: short.clone(),
            source: source_arc,
            line_starts: line_starts.into(),
            detected_name: Some(short),
            cached_galley: None,
            read_only: true,
            library: crate::ui::state::ModelLibrary::MSL,
            doc: Some(doc_id),
        });
        ws.diagram_dirty = true;
    }

    // Open a new model-view tab.
    {
        let mut model_tabs =
            world.resource_mut::<crate::ui::panels::model_view::ModelTabs>();
        model_tabs.ensure(doc_id);
    }
    if let Some(mut layout) =
        world.get_resource_mut::<lunco_workbench::WorkbenchLayout>()
    {
        layout.open_instance(
            crate::ui::panels::model_view::MODEL_VIEW_KIND,
            doc_id.raw(),
        );
    }

    bevy::log::info!(
        "[CanvasDiagram] drill-in: opened `{}` from `{}`",
        qualified,
        file_path.display()
    );
}

// ─── Doc-op translation ─────────────────────────────────────────────

/// Resolve `(document id, editing class name)` for the current tab.
/// Mirrors the snarl panel's logic so both panels target the same
/// class when `open_model` is bound.
fn resolve_doc_context(world: &World) -> (Option<lunco_doc::DocumentId>, Option<String>) {
    let Some(open) = world.resource::<WorkbenchState>().open_model.as_ref() else {
        return (None, None);
    };
    let Some(doc_id) = open.doc else {
        return (None, None);
    };
    let class = world
        .resource::<ModelicaDocumentRegistry>()
        .host(doc_id)
        .and_then(|h| {
            h.document()
                .ast()
                .ast()
                .and_then(|s| s.classes.keys().next().cloned())
        })
        .or_else(|| open.detected_name.clone());
    (Some(doc_id), class)
}

// Thin wrapper so existing call sites keep their shape. The real
// conversion lives in `coords::canvas_min_to_modelica_center`.
fn canvas_min_to_modelica_center(min: lunco_canvas::Pos) -> (f32, f32) {
    let m = coords::canvas_min_to_modelica_center(min, ICON_W, ICON_H);
    (m.x, m.y)
}

/// Translate canvas scene events into ModelicaOps. Needs a brief
/// read-only borrow of the scene (to look up edge endpoints); the
/// caller runs it inside its own borrow scope.
fn build_ops_from_events(
    world: &mut World,
    events: &[lunco_canvas::SceneEvent],
    class: &str,
) -> Vec<ModelicaOp> {
    use lunco_canvas::SceneEvent;
    let state = world.resource::<CanvasDiagramState>();
    let scene = &state.canvas.scene;
    let mut ops: Vec<ModelicaOp> = Vec::new();

    for ev in events {
        match ev {
            SceneEvent::NodeMoved { id, new_min, .. } => {
                // The `origin` we set during projection carries the
                // Modelica instance name. Skip if missing (shouldn't
                // happen — projection always sets it).
                let Some(node) = scene.node(*id) else { continue };
                let Some(name) = node.origin.clone() else { continue };
                let (mx, my) = canvas_min_to_modelica_center(*new_min);
                ops.push(ModelicaOp::SetPlacement {
                    class: class.to_string(),
                    name,
                    placement: Placement::at(mx, my),
                });
            }
            SceneEvent::EdgeCreated { from, to } => {
                // Resolve canvas port refs → Modelica (instance,
                // port) pairs via node.origin + port.id.
                let Some(from_node) = scene.node(from.node) else { continue };
                let Some(to_node) = scene.node(to.node) else { continue };
                let Some(from_instance) = from_node.origin.clone() else { continue };
                let Some(to_instance) = to_node.origin.clone() else { continue };
                ops.push(ModelicaOp::AddConnection {
                    class: class.to_string(),
                    eq: pretty::ConnectEquation {
                        from: pretty::PortRef::new(&from_instance, from.port.as_str()),
                        to: pretty::PortRef::new(&to_instance, to.port.as_str()),
                        line: None,
                    },
                });
            }
            SceneEvent::EdgeDeleted { id } => {
                if let Some(op) = op_remove_edge_inner(scene, *id, class) {
                    ops.push(op);
                }
            }
            SceneEvent::NodeDeleted { id, orphaned_edges } => {
                // Orphan edge RemoveConnection ops must go in
                // BEFORE the RemoveComponent so rumoca still sees
                // the edges while resolving the connect(...) spans.
                for eid in orphaned_edges {
                    if let Some(op) = op_remove_edge_inner(scene, *eid, class) {
                        ops.push(op);
                    }
                }
                if let Some(op) = op_remove_node_inner(scene, *id, class) {
                    ops.push(op);
                }
            }
            _ => {}
        }
    }
    ops
}

/// `(instance_name, type_label)` for a node, pulled from the scene's
/// `label` + `data.type`. Empty strings when the node is gone.
fn component_headers(
    world: &World,
    id: lunco_canvas::NodeId,
) -> (String, String) {
    let state = world.resource::<CanvasDiagramState>();
    let Some(node) = state.canvas.scene.node(id) else {
        return (String::new(), String::new());
    };
    let instance = node.label.clone();
    let type_name = node
        .data
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    (instance, type_name)
}

/// Build an `AddComponent` op at a world-space position. Carries
/// the component's default parameter values and a `Placement`
/// annotation so the new node lands at the right spot in both the
/// source and any downstream re-projection.
fn op_add_component(
    comp: &MSLComponentDef,
    at_world: lunco_canvas::Pos,
    class: &str,
) -> ModelicaOp {
    // `at_world` is the click position — already the intended
    // centre, not a rect min — so we don't add the icon offsets
    // here. Just flip canvas → Modelica via the typed conversion.
    let ModelicaPos { x: mx, y: my } = canvas_to_modelica(at_world);
    // Auto-generate a unique instance name: first letter of the
    // component's short name + a counter. VisualDiagram's own
    // `next_instance_name` does this but requires a mutable
    // VisualDiagram instance — for our static-ops path we just use
    // a timestamp-ish fallback. B4: snapshot the doc to count
    // existing instances and pick the next N.
    let prefix = comp
        .name
        .chars()
        .next()
        .unwrap_or('X')
        .to_ascii_uppercase();
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_millis() % 10_000)
        .unwrap_or(0);
    let instance_name = format!("{}{}", prefix, suffix);
    ModelicaOp::AddComponent {
        class: class.to_string(),
        decl: pretty::ComponentDecl {
            type_name: comp.msl_path.clone(),
            name: instance_name,
            modifications: comp
                .parameters
                .iter()
                .filter(|p| !p.default.is_empty())
                .map(|p| (p.name.clone(), p.default.clone()))
                .collect(),
            placement: Some(Placement::at(mx, my)),
        },
    }
}

fn op_remove_component(
    world: &mut World,
    id: lunco_canvas::NodeId,
    class: &str,
) -> Option<ModelicaOp> {
    let state = world.resource::<CanvasDiagramState>();
    op_remove_node_inner(&state.canvas.scene, id, class)
}

fn op_remove_edge(
    world: &mut World,
    id: lunco_canvas::EdgeId,
    class: &str,
) -> Option<ModelicaOp> {
    let state = world.resource::<CanvasDiagramState>();
    op_remove_edge_inner(&state.canvas.scene, id, class)
}

fn op_remove_node_inner(
    scene: &lunco_canvas::Scene,
    id: lunco_canvas::NodeId,
    class: &str,
) -> Option<ModelicaOp> {
    let node = scene.node(id)?;
    let name = node.origin.clone()?;
    Some(ModelicaOp::RemoveComponent {
        class: class.to_string(),
        name,
    })
}

fn op_remove_edge_inner(
    scene: &lunco_canvas::Scene,
    id: lunco_canvas::EdgeId,
    class: &str,
) -> Option<ModelicaOp> {
    let edge = scene.edge(id)?;
    let from_node = scene.node(edge.from.node)?;
    let to_node = scene.node(edge.to.node)?;
    let from_instance = from_node.origin.clone()?;
    let to_instance = to_node.origin.clone()?;
    Some(ModelicaOp::RemoveConnection {
        class: class.to_string(),
        from: pretty::PortRef::new(&from_instance, edge.from.port.as_str()),
        to: pretty::PortRef::new(&to_instance, edge.to.port.as_str()),
    })
}

/// Apply a batch of ops against the bound document. Ops that fail
/// (e.g. RemoveComponent when the instance isn't actually in source
/// — shouldn't happen, but defence in depth) are logged and
/// skipped. After success the doc's generation bumps, which the
/// next frame picks up via `last_seen_gen` and re-projects.
fn apply_ops(world: &mut World, doc_id: lunco_doc::DocumentId, ops: Vec<ModelicaOp>) {
    let n = ops.len();
    let mut any_applied = false;
    {
        let Some(mut registry) = world.get_resource_mut::<ModelicaDocumentRegistry>() else {
            bevy::log::warn!(
                "[CanvasDiagram] tried to apply {} op(s) but registry missing",
                n
            );
            return;
        };
        let Some(host) = registry.host_mut(doc_id) else {
            bevy::log::warn!(
                "[CanvasDiagram] tried to apply {} op(s) but doc {:?} not in registry",
                n,
                doc_id
            );
            return;
        };
        for op in ops {
            bevy::log::info!("[CanvasDiagram] applying {:?}", op);
            match host.apply(op) {
                Ok(_) => any_applied = true,
                Err(e) => bevy::log::warn!("[CanvasDiagram] op failed: {}", e),
            }
        }
    }

    if !any_applied {
        return;
    }

    // Mirror the post-edit source back to `WorkbenchState.open_model`
    // so every other panel (code editor, breadcrumb, inspector)
    // that reads the cached source sees the update immediately —
    // the code editor doesn't watch the registry directly; it
    // reads the `Arc<str>` on `open_model`. Matches the snarl
    // panel's write-back path.
    let fresh = world
        .get_resource::<ModelicaDocumentRegistry>()
        .and_then(|r| r.host(doc_id))
        .map(|h| {
            (
                h.document().source().to_string(),
                <crate::document::ModelicaDocument as lunco_doc::Document>::generation(
                    h.document(),
                ),
            )
        });
    if let Some((src, _new_gen)) = fresh {
        if let Some(mut ws) = world.get_resource_mut::<WorkbenchState>() {
            if let Some(open) = ws.open_model.as_mut() {
                let mut line_starts = vec![0usize];
                for (i, b) in src.as_bytes().iter().enumerate() {
                    if *b == b'\n' {
                        line_starts.push(i + 1);
                    }
                }
                open.source = std::sync::Arc::from(src.as_str());
                open.line_starts = line_starts.into();
                open.cached_galley = None;
            }
        }
        // IMPORTANT: do NOT advance `last_seen_gen` here. Letting
        // the next-frame project check see the bumped generation
        // triggers a fresh projection, which is how the newly-added
        // node / edge actually shows up on the canvas. Snarl skips
        // re-projection because its viewer mutates snarl state in
        // lock-step with the op, but we don't — we always project
        // from the document source, so skipping re-projection
        // leaves the canvas scene stale.
    }
}
