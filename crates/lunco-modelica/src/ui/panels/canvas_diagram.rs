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
use crate::visual_diagram::{DiagramNodeId, MSLComponentDef, VisualDiagram};
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
    /// Pure-icon class (zero connectors, `.Icons.*` subpackage).
    /// Rendered with a dashed border so users can tell at a glance
    /// the component is decorative. Set by the projector via the
    /// node's `data.icon_only` flag.
    icon_only: bool,
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

        // Always paint a solid card background *underneath* the SVG.
        // Why: MSL icons are outlined shapes — the SVG pixels inside
        // the outline are transparent by design, so without a bg the
        // connection lines running behind an icon are visible through
        // its body. That reads as "the diagram is a sheet of glass"
        // rather than "icons are opaque tiles." Dymola/OMEdit both
        // paint each icon on its own opaque card for the same reason.
        let card_fill = egui::Color32::from_rgb(48, 56, 72);
        painter.rect_filled(rect, 6.0, card_fill);

        let mut drew_svg = false;
        if !self.icon_asset.is_empty() {
            if let Some(bytes) = svg_bytes_for(&self.icon_asset) {
                super::svg_renderer::draw_svg_to_egui(painter, rect, &bytes);
                drew_svg = true;
            }
        }

        if !drew_svg {
            // SVG missing / failed to load: the card is already
            // painted; just add a type label so the user still sees
            // something meaningful instead of a blank box.
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
        // visible even over busy SVG content. Icon-only classes
        // (no connectors, visual-only) get a dashed border instead
        // of solid — a signal that the component isn't hookable.
        let stroke = if selected {
            egui::Stroke::new(2.0, egui::Color32::from_rgb(120, 170, 255))
        } else if self.icon_only {
            egui::Stroke::new(1.0, egui::Color32::from_rgb(150, 130, 90))
        } else {
            egui::Stroke::new(1.0, egui::Color32::from_rgb(90, 100, 120))
        };
        if self.icon_only && !selected {
            // Dashed border via four side-segments sampled in
            // short dash+gap runs. Cheap (12-16 line_segment calls
            // per node) and looks right at all zoom levels because
            // we dash in screen pixels here.
            paint_dashed_rect(painter, rect, 6.0, stroke);
        } else {
            painter.rect_stroke(rect, 6.0, stroke, egui::StrokeKind::Outside);
        }

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

/// Paint a dashed rectangle outline. Used for icon-only classes so
/// users see at a glance that the node is decorative (no
/// connectors). Dashes are expressed in screen pixels because the
/// caller has already transformed to screen-space — so the dash
/// pattern stays the same visual size regardless of zoom. `radius`
/// is currently unused (corners are sampled as-if straight for
/// simplicity); revisit if the corner elision gets noticed.
fn paint_dashed_rect(
    painter: &egui::Painter,
    rect: egui::Rect,
    _radius: f32,
    stroke: egui::Stroke,
) {
    let dash_len = 4.0;
    let gap_len = 3.0;
    let period = dash_len + gap_len;
    // Walk each of the four edges, emitting dash-sized segments.
    let edges = [
        (rect.min, egui::pos2(rect.max.x, rect.min.y)), // top
        (egui::pos2(rect.max.x, rect.min.y), rect.max), // right
        (rect.max, egui::pos2(rect.min.x, rect.max.y)), // bottom
        (egui::pos2(rect.min.x, rect.max.y), rect.min), // left
    ];
    for (a, b) in edges {
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < f32::EPSILON {
            continue;
        }
        let ux = dx / len;
        let uy = dy / len;
        let mut t = 0.0_f32;
        while t < len {
            let end = (t + dash_len).min(len);
            painter.line_segment(
                [
                    egui::pos2(a.x + ux * t, a.y + uy * t),
                    egui::pos2(a.x + ux * end, a.y + uy * end),
                ],
                stroke,
            );
            t += period;
        }
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
        // `type` is the fully-qualified path (used by drill-in);
        // show only its tail under the icon so the label isn't a
        // 50-character package path.
        let qualified = data
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let type_label = qualified.rsplit('.').next().unwrap_or(qualified).to_string();
        let icon_asset = data
            .get("icon_asset")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let icon_only = data
            .get("icon_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        IconNodeVisual {
            type_label,
            icon_asset,
            icon_only,
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
                // Full qualified name — what the drill-in resolver
                // feeds into Modelica's package layout lookup. The
                // short `name` is fine for labels, but breaks
                // drill-in (which needs the path) and the type
                // hint shown under the label.
                "type": node.component_def.msl_path,
                "icon_asset": node.component_def.icon_asset.clone().unwrap_or_default(),
                // Flag pure-icon classes so the renderer can draw
                // them with a dashed border — users see at a
                // glance that these are decorative and have no
                // connectors to hook up.
                "icon_only": crate::class_cache::is_icon_only_class(
                    &node.component_def.msl_path,
                ),
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

/// Per-document canvas state. Each open model tab owns one of
/// these, keyed by [`DocumentId`] on [`CanvasDiagramState`]. Holds
/// the transform + selection + in-flight projection task for that
/// specific document so switching tabs doesn't leak viewport,
/// selection, or a stale projection into a neighbour.
pub struct CanvasDocState {
    pub canvas: Canvas,
    pub last_seen_gen: u64,
    pub context_menu: Option<PendingContextMenu>,
    pub projection_task: Option<ProjectionTask>,
}

impl Default for CanvasDocState {
    fn default() -> Self {
        let mut canvas = Canvas::new(build_registry());
        canvas.layers.retain(|layer| layer.name() != "selection");
        canvas.overlays.push(Box::new(NavBarOverlay::default()));
        Self {
            canvas,
            last_seen_gen: 0,
            context_menu: None,
            projection_task: None,
        }
    }
}

/// Per-panel state carried across frames. Stored as a Bevy resource
/// so the panel's `render` can pull it out via `world.resource_mut`.
///
/// State is sharded per-document — each open model tab has its own
/// [`CanvasDocState`] entry so viewport/selection/projection/context
/// menu never bleed between tabs. `fallback` is used only when no
/// document is bound (startup, every tab closed).
#[derive(Resource, Default)]
pub struct CanvasDiagramState {
    per_doc: std::collections::HashMap<lunco_doc::DocumentId, CanvasDocState>,
    fallback: CanvasDocState,
}

impl CanvasDiagramState {
    /// Read-only view of the state for a given doc. Falls back to an
    /// empty canvas when `doc` is `None` or no entry exists yet — used
    /// during the one-frame window between panel mount and first
    /// projection.
    pub fn get(&self, doc: Option<lunco_doc::DocumentId>) -> &CanvasDocState {
        doc.and_then(|d| self.per_doc.get(&d)).unwrap_or(&self.fallback)
    }

    /// Mutable view, creating the entry on first access. `None` routes
    /// writes to the shared fallback so "no doc bound" doesn't crash.
    pub fn get_mut(
        &mut self,
        doc: Option<lunco_doc::DocumentId>,
    ) -> &mut CanvasDocState {
        match doc {
            Some(d) => self.per_doc.entry(d).or_default(),
            None => &mut self.fallback,
        }
    }

    /// Drop a doc's entry when its document is removed from the
    /// registry (tab closed, file unloaded). Called from
    /// [`cleanup_removed_documents`].
    pub fn drop_doc(&mut self, doc: lunco_doc::DocumentId) {
        self.per_doc.remove(&doc);
    }

    /// Has this doc ever been projected? `false` until
    /// `get_mut(Some(doc))` inserts — the trigger the render loop
    /// uses to force an initial projection.
    pub fn has_entry(&self, doc: lunco_doc::DocumentId) -> bool {
        self.per_doc.contains_key(&doc)
    }
}

/// Running projection task + the generation that spawned it, so the
/// poll loop can tell whether we've moved on since and should drop a
/// stale result. The owning doc is implicit: each task lives on that
/// doc's [`CanvasDocState`].
///
/// # Cancellation
///
/// Bevy tasks can't be preempted — "cancel" is cooperative. We
/// give the task a shared `AtomicBool` and a deadline; it polls
/// them at phase boundaries (build → edges recovery → project)
/// and returns an empty `Scene` if either fires. The poll loop
/// drops the handle when the deadline elapses even if the task
/// hasn't noticed yet — the pool runs it to completion but nobody
/// reads the result.
///
/// Two independent "stop" signals:
///
/// - **`cancel`** — flipped to `true` explicitly (user hits
///   cancel, new generation supersedes, tab closed, etc.).
/// - **`deadline`** — wall-clock elapsed > configured max. Reads
///   live via `spawned_at.elapsed() > deadline`.
pub struct ProjectionTask {
    pub gen_at_spawn: u64,
    pub spawned_at: std::time::Instant,
    pub deadline: std::time::Duration,
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub task: bevy::tasks::Task<Scene>,
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

        // Decide whether to rebuild the scene. Per-doc state means
        // "bound_doc" is implicit in the map key — a fresh entry has
        // `last_seen_gen == 0` so the first render after tab open
        // always re-projects.
        let project_now = {
            let Some(open) = world.resource::<WorkbenchState>().open_model.clone() else {
                // No model open — reset fallback canvas and bail.
                world
                    .resource_mut::<CanvasDiagramState>()
                    .get_mut(None)
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
            // Two triggers for projection:
            //   1. **First render of this tab** — `has_entry` is
            //      false. MSL library docs land with generation 0
            //      and our fresh state cursor also starts at 0, so a
            //      gen-only check would never fire and the drilled-
            //      in canvas would stay blank forever. Insert-on-
            //      first-render is the right fix.
            //   2. **Doc mutated** — generation bumped past
            //      `last_seen_gen`. Standard edit-reproject path.
            let docstate = state.get(Some(doc_id));
            let first_render = !state.has_entry(doc_id);
            let gen_advanced = gen != docstate.last_seen_gen;
            (first_render || gen_advanced).then_some((doc_id, gen))
        };

        if let Some((doc_id, gen)) = project_now {
            // Spawn a background task (or reuse an in-flight one
            // for the same doc+gen) that runs edge-recovery and
            // builds a `Scene` from the document's already-parsed
            // AST — no re-parse. Hot path: clone the `Arc<StoredDefinition>`
            // (cheap) + clone the source (byte copy) and ship both
            // to the task. `import_model_to_diagram_from_ast` avoids
            // the two full rumoca passes `import_model_to_diagram`
            // used to run.
            let (source, ast_arc) = {
                let registry = world.resource::<ModelicaDocumentRegistry>();
                let Some(host) = registry.host(doc_id) else {
                    return;
                };
                let doc = host.document();
                let ast = doc.ast().result.as_ref().ok().cloned();
                (doc.source().to_string(), ast)
            };
            // Snapshot the configurable projection caps so the bg
            // task doesn't need to reach back into the world (it
            // can't — it runs off-thread with only owned data).
            let (max_nodes_snapshot, max_duration_snapshot) = world
                .get_resource::<DiagramProjectionLimits>()
                .map(|l| (l.max_nodes, l.max_duration))
                .unwrap_or((
                    crate::ui::panels::diagram::DEFAULT_MAX_DIAGRAM_NODES,
                    std::time::Duration::from_secs(60),
                ));
            // Target class for the projection: the fully-qualified
            // name the drill-in tab points at. Read from
            // `DrilledInClassNames`, which the drill-in install
            // populated and which persists for the tab's lifetime.
            // Reading `open_model.model_path` doesn't work here —
            // for installed docs it's the filesystem path, not the
            // `msl://…` URI. `None` for Untitled / user-authored
            // docs — builder picks the first non-package class as
            // before.
            let target_class_snapshot: Option<String> = world
                .get_resource::<DrilledInClassNames>()
                .and_then(|m| m.get(doc_id).map(str::to_string));
            let mut state = world.resource_mut::<CanvasDiagramState>();
            let docstate = state.get_mut(Some(doc_id));
            // Drop any in-flight projection whose input is now
            // stale (older generation of this doc). We can't cancel
            // a `Task` cleanly in Bevy's API, but dropping the
            // handle releases our interest — the pool still runs it
            // to completion, the result is just thrown away when we
            // poll. Cross-doc staleness is no longer possible now
            // that tasks live on per-doc state.
            let stale = match &docstate.projection_task {
                Some(t) => t.gen_at_spawn != gen,
                None => false,
            };
            if stale {
                docstate.projection_task = None;
            }
            if docstate.projection_task.is_none() {
                // Hard ceiling: the projection path is now
                // `Arc<StoredDefinition>`-based (no deep clone), so
                // MB-scale ASTs are no longer an OOM risk. The cap
                // below is a never-freeze guarantee against pathological
                // inputs (gigabyte sources, etc.) — not a routine
                // throttle. Tune only if users actually hit it. The
                // per-class graph cap (`DiagramProjectionLimits::max_nodes`,
                // user-configurable) catches the "this file parsed
                // fine but has too many components to show usefully"
                // case.
                const PROJECTION_SOURCE_HARD_CEILING: usize = 10_000_000; // 10 MB
                let source_len = source.len();
                let skip_projection = source_len > PROJECTION_SOURCE_HARD_CEILING;
                if skip_projection {
                    bevy::log::warn!(
                        "[CanvasDiagram] refusing to project: source {} bytes \
                         exceeds the {} hard ceiling. Use Text view.",
                        source_len,
                        PROJECTION_SOURCE_HARD_CEILING,
                    );
                    // Mark as "seen at this gen" so the render loop
                    // doesn't keep retrying every frame.
                    docstate.last_seen_gen = gen;
                } else {
                    let pool = bevy::tasks::AsyncComputeTaskPool::get();
                    let spawned_at = std::time::Instant::now();
                    let cancel = std::sync::Arc::new(
                        std::sync::atomic::AtomicBool::new(false),
                    );
                    let cancel_for_task = std::sync::Arc::clone(&cancel);
                    let deadline = max_duration_snapshot;
                    let target_for_log = target_class_snapshot.clone();
                    let source_bytes_for_log = source.len();
                    let task = pool.spawn(async move {
                        use std::sync::atomic::Ordering;
                        let should_stop = || {
                            cancel_for_task.load(Ordering::Relaxed)
                                || spawned_at.elapsed() > deadline
                        };
                        bevy::log::info!(
                            "[Projection] start: {} bytes target={:?}",
                            source_bytes_for_log,
                            target_for_log,
                        );
                        if should_stop() {
                            return Scene::new();
                        }
                        let t0 = std::time::Instant::now();
                        let mut diagram = if let Some(ast) = ast_arc {
                            crate::ui::panels::diagram::import_model_to_diagram_from_ast(
                                ast,
                                &source,
                                max_nodes_snapshot,
                                target_for_log.as_deref(),
                            )
                            .unwrap_or_default()
                        } else {
                            crate::ui::panels::diagram::import_model_to_diagram(&source)
                                .unwrap_or_default()
                        };
                        bevy::log::info!(
                            "[Projection] import done in {:.0}ms: {} nodes {} edges",
                            t0.elapsed().as_secs_f64() * 1000.0,
                            diagram.nodes.len(),
                            diagram.edges.len(),
                        );
                        if should_stop() {
                            return Scene::new();
                        }
                        let t1 = std::time::Instant::now();
                        recover_edges_from_source(&source, &mut diagram);
                        bevy::log::info!(
                            "[Projection] recover_edges done in {:.0}ms: {} edges",
                            t1.elapsed().as_secs_f64() * 1000.0,
                            diagram.edges.len(),
                        );
                        if should_stop() {
                            return Scene::new();
                        }
                        let t2 = std::time::Instant::now();
                        let (scene, _id_map) = project_scene(&diagram);
                        bevy::log::info!(
                            "[Projection] project_scene done in {:.0}ms",
                            t2.elapsed().as_secs_f64() * 1000.0,
                        );
                        scene
                    });
                    docstate.projection_task = Some(ProjectionTask {
                        gen_at_spawn: gen,
                        spawned_at,
                        deadline,
                        cancel,
                        task,
                    });
                }
            }
            // DO NOT update last_seen_gen here — only after the
            // task completes and the scene is actually swapped in.
            // Otherwise the `project_now` check on later frames
            // would think we're up-to-date and never swap.
            let _ = state;
        }

        // Poll the in-flight projection task for the ACTIVE doc.
        // When it finishes, swap the scene in, update the sync
        // cursor, and (on first projection for this tab) frame the
        // scene with a sensible initial zoom.
        {
            let active_doc = world
                .resource::<WorkbenchState>()
                .open_model
                .as_ref()
                .and_then(|m| m.doc);
            // Pre-fetch current gen from the registry before we
            // take the mutable borrow of CanvasDiagramState, so we
            // can use it inside the deadline-guard block below
            // without fighting borrow rules.
            let current_gen_for_deadline = active_doc.and_then(|d| {
                world
                    .get_resource::<ModelicaDocumentRegistry>()
                    .and_then(|r| r.host(d))
                    .map(|h| h.document().generation())
            });
            let mut state = world.resource_mut::<CanvasDiagramState>();
            let docstate = state.get_mut(active_doc);
            let is_initial_projection = docstate.last_seen_gen == 0;

            // Deadline guard. If the task has been running past its
            // configured budget, flip its cancel flag and drop the
            // handle. The pool still runs the task to completion
            // (Bevy tasks can't be preempted), but the cooperative
            // `should_stop` check inside the task short-circuits
            // the remaining phases and nobody waits on the result.
            // We mark `last_seen_gen = current_gen` so the render
            // loop doesn't respawn the same doomed task next frame;
            // the user has to edit the doc (generation bump) to
            // retry — which is the correct recovery action.
            let timed_out = docstate
                .projection_task
                .as_ref()
                .map(|t| t.spawned_at.elapsed() > t.deadline)
                .unwrap_or(false);
            if timed_out {
                use std::sync::atomic::Ordering;
                if let Some(t) = docstate.projection_task.as_ref() {
                    t.cancel.store(true, Ordering::Relaxed);
                    bevy::log::warn!(
                        "[CanvasDiagram] projection exceeded {:.1}s deadline \
                         — cancelled. Raise Settings → Diagram → Timeout \
                         to allow longer.",
                        t.deadline.as_secs_f32(),
                    );
                }
                docstate.projection_task = None;
                if let Some(g) = current_gen_for_deadline {
                    docstate.last_seen_gen = g;
                }
            }

            let done_task = docstate
                .projection_task
                .as_mut()
                .and_then(|t| {
                    futures_lite::future::block_on(
                        futures_lite::future::poll_once(&mut t.task),
                    )
                    .map(|scene| (t.gen_at_spawn, scene))
                });
            if let Some((gen, scene)) = done_task {
                docstate.projection_task = None;
                bevy::log::info!(
                    "[CanvasDiagram] project done: {} nodes, {} edges (initial={})",
                    scene.node_count(),
                    scene.edge_count(),
                    is_initial_projection,
                );
                docstate.canvas.scene = scene;
                docstate.canvas.selection.clear();
                docstate.last_seen_gen = gen;
                if is_initial_projection {
                    let physical_zoom =
                        lunco_canvas::Viewport::physical_mm_zoom(ui.ctx());
                    if let Some(world_rect) = docstate.canvas.scene.bounds() {
                        let screen = lunco_canvas::Rect::from_min_max(
                            lunco_canvas::Pos::new(0.0, 0.0),
                            lunco_canvas::Pos::new(800.0, 600.0),
                        );
                        let (c, z) = docstate
                            .canvas
                            .viewport
                            .fit_values(world_rect, screen, 40.0);
                        let z = z.min(physical_zoom * 2.0).max(physical_zoom * 0.5);
                        docstate.canvas.viewport.snap_to(c, z);
                    } else {
                        docstate.canvas.viewport.snap_to(
                            lunco_canvas::Pos::new(0.0, 0.0),
                            physical_zoom,
                        );
                    }
                }
                // A projection just finished — request a repaint so
                // the user sees the new scene immediately rather
                // than on the next input tick.
                ui.ctx().request_repaint();
            } else if docstate.projection_task.is_some() {
                // Still parsing — repaint so the "Projecting…"
                // indicator animates smoothly.
                ui.ctx().request_repaint();
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

        // Active doc — the tab whose canvas should respond to
        // input this frame. All state accesses below route through
        // this id so neighbour tabs stay untouched.
        let active_doc = doc_id;

        // Read-only library class (MSL, imported file the user
        // opened via drill-in) — no editing gestures should take
        // effect here. We gate the whole right-click menu on this
        // so readonly tabs don't even offer "Add component" etc.;
        // the canvas itself stays fully navigable (pan/zoom/select).
        let tab_read_only = world
            .resource::<WorkbenchState>()
            .open_model
            .as_ref()
            .map(|m| m.read_only)
            .unwrap_or(false);

        // Render the canvas and collect its events.
        let (response, events) = {
            let mut state = world.resource_mut::<CanvasDiagramState>();
            state.get_mut(active_doc).canvas.ui(ui)
        };

        // Overlay state machine, in priority order:
        //   1. Drill-in load in flight → "Loading <class>…" card.
        //      Highest priority because the document is a placeholder
        //      and anything else (empty summary, etc.) would
        //      misrepresent what's going on.
        //   2. Projection task in flight → "Projecting…" spinner.
        //   3. Empty scene, no task → equation-only model summary.
        let (loading_info, projecting, show_empty_overlay, scene_has_content) = {
            let state = world.resource::<CanvasDiagramState>();
            let loads = world.resource::<DrillInLoads>();
            let docstate = state.get(active_doc);
            let info = active_doc
                .and_then(|d| loads.progress(d))
                .map(|(q, secs)| (q.to_string(), secs));
            let has_content = docstate.canvas.scene.node_count() > 0;
            (
                info,
                docstate.projection_task.is_some(),
                !has_content && docstate.projection_task.is_none(),
                has_content,
            )
        };
        // Loading overlay: only on tabs that are genuinely waiting
        // on a drill-in parse (and the scene hasn't been populated
        // yet). Once the scene has content, any brief re-projection
        // from an edit swaps atomically without flashing.
        if let Some((class, secs)) = loading_info {
            if !scene_has_content {
                render_drill_in_loading_overlay(ui, response.rect, &class, secs);
            }
        } else if projecting && !scene_has_content {
            render_projecting_overlay(ui, response.rect);
        } else if show_empty_overlay {
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
        let mut suppress_menu = tab_read_only;

        if tab_read_only {
            // Belt-and-braces: if egui has a popup cached from a
            // previous (editable) tab, close it so switching tabs
            // doesn't leave an orphan menu around. Cheap no-op when
            // nothing is open.
            if popup_was_open_before {
                ui.ctx().memory_mut(|m| m.close_all_popups());
            }
            world
                .resource_mut::<CanvasDiagramState>()
                .get_mut(active_doc)
                .context_menu = None;
        }

        if !tab_read_only && response.secondary_clicked() {
            let press = ui.ctx().input(|i| i.pointer.press_origin());
            if let Some(p) = press.or_else(|| response.interact_pointer_pos()) {
                // Only treat as "dismiss" if this tab itself has a
                // cached menu open. egui's global popup memory can
                // carry a stale popup from a tab we just switched
                // away from (readonly → editable); without this
                // check the first right-click on the new tab gets
                // eaten as a dismiss and the user has to click
                // twice.
                let our_menu_open = popup_was_open_before
                    && world
                        .resource::<CanvasDiagramState>()
                        .get(active_doc)
                        .context_menu
                        .is_some();
                if our_menu_open {
                    // Second right-click while the menu is up → dismiss.
                    // We BOTH clear our cache AND ask egui to close
                    // any popup. Skipping `context_menu` below prevents
                    // egui from re-opening on this same frame.
                    world
                        .resource_mut::<CanvasDiagramState>()
                        .get_mut(active_doc)
                        .context_menu = None;
                    ui.ctx().memory_mut(|m| m.close_all_popups());
                    suppress_menu = true;
                } else {
                    // If egui still thinks a popup is open (from a
                    // previous tab), close it so this frame's
                    // `response.context_menu()` can open our fresh
                    // one without egui deduping against the stale
                    // popup id.
                    if popup_was_open_before {
                        ui.ctx().memory_mut(|m| m.close_all_popups());
                    }
                    // Fresh right-click: capture world position +
                    // hit-test origin while `press_origin` still
                    // reflects the right-click (before any menu-entry
                    // click overwrites it).
                    let state = world.resource::<CanvasDiagramState>();
                    let docstate = state.get(active_doc);
                    let world_pos = docstate.canvas.viewport.screen_to_world(
                        lunco_canvas::Pos::new(p.x, p.y),
                        screen_rect,
                    );
                    let hit_node = docstate.canvas.scene.hit_node(world_pos, 6.0);
                    let hit_edge = docstate.canvas.scene.hit_edge(world_pos, 4.0);
                    let target = match (hit_node, hit_edge) {
                        (Some((id, _)), _) => ContextMenuTarget::Node(id),
                        (_, Some(id)) => ContextMenuTarget::Edge(id),
                        _ => ContextMenuTarget::Empty,
                    };
                    let _ = state;
                    bevy::log::info!(
                        "[CanvasDiagram] right-click screen=({:.1},{:.1}) world=({:.1},{:.1}) target={:?}",
                        p.x, p.y, world_pos.x, world_pos.y, target
                    );
                    world
                        .resource_mut::<CanvasDiagramState>()
                        .get_mut(active_doc)
                        .context_menu = Some(PendingContextMenu {
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
                .get(active_doc)
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
                .get(active_doc)
                .context_menu
                .is_some()
        {
            world
                .resource_mut::<CanvasDiagramState>()
                .get_mut(active_doc)
                .context_menu = None;
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
                        .get(active_doc)
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

// ─── MSL package tree (for nested add-component menu) ──────────────

/// One node in the MSL package hierarchy. `classes` are instantiable
/// at this level (instances we'd add to the diagram), `subpackages`
/// are deeper navigation. `BTreeMap` for stable alphabetical order
/// regardless of the source list's order.
struct MslPackageNode {
    subpackages: std::collections::BTreeMap<String, MslPackageNode>,
    classes: Vec<&'static MSLComponentDef>,
}

impl MslPackageNode {
    fn new() -> Self {
        Self {
            subpackages: Default::default(),
            classes: Vec::new(),
        }
    }
}

/// User-facing toggles for the MSL add-component menu. Default
/// values are tuned for the common case ("a user dropping a
/// component expects a functional block, not an icon shell").
/// Persisted as a Bevy resource; the Settings dropdown flips the
/// `show_icon_only_classes` flag to override.
#[derive(Resource, Debug, Clone)]
pub struct PaletteSettings {
    /// When `true`, pure-icon classes (matched by
    /// [`crate::class_cache::is_icon_only_class`]) appear in the
    /// MSL add-component submenus. Default `false` — matches
    /// Dymola's "hide `.Icons.*`" default.
    pub show_icon_only_classes: bool,
}

impl Default for PaletteSettings {
    fn default() -> Self {
        Self {
            show_icon_only_classes: false,
        }
    }
}

/// Soft guards for the canvas projection. Prevent accidental
/// attempts to diagram huge packages without getting in the way of
/// deeply composed real models. Exposed via the Settings dropdown.
#[derive(Resource, Debug, Clone)]
pub struct DiagramProjectionLimits {
    /// Maximum component count the projector will accept before
    /// returning `None`. Default
    /// [`crate::ui::panels::diagram::DEFAULT_MAX_DIAGRAM_NODES`]
    /// (1000). Users building power-system or multi-body models
    /// with hundreds of components can raise this in Settings.
    pub max_nodes: usize,
    /// Wall-clock deadline for a single projection task. If the bg
    /// task hasn't resolved within this window, the poll loop
    /// flips the task's `cancel` flag AND drops the handle. Task
    /// finishes (waste, but bounded), result is discarded, canvas
    /// stays empty with a "projection timed out" overlay.
    ///
    /// Deliberately high (60 s default) — only catches truly
    /// catastrophic work, not normal drill-ins. Raise in Settings
    /// if you're profiling something slow on purpose.
    pub max_duration: std::time::Duration,
}

impl Default for DiagramProjectionLimits {
    fn default() -> Self {
        Self {
            max_nodes: crate::ui::panels::diagram::DEFAULT_MAX_DIAGRAM_NODES,
            max_duration: std::time::Duration::from_secs(60),
        }
    }
}

/// True if the subtree contains any class that would be visible
/// with the icon-only filter ON. Used to prune empty submenus at
/// render time so the user doesn't click into a dead-end
/// `Mechanics > Rotational > Icons` branch.
///
/// Recursive but cheap — MSL is ~2400 classes across a shallow
/// tree (depth ≤ 6). The menu builder hits this at most once per
/// submenu when opened.
fn package_has_visible_classes(node: &MslPackageNode) -> bool {
    if node
        .classes
        .iter()
        .any(|c| !crate::class_cache::is_icon_only_class(&c.msl_path))
    {
        return true;
    }
    node.subpackages
        .values()
        .any(package_has_visible_classes)
}

/// Lazily-built package tree. Walks every entry in
/// [`crate::visual_diagram::msl_component_library`] once and
/// inserts it under its dotted package path. Cached for the life
/// of the process — MSL content doesn't change at runtime.
fn msl_package_tree() -> &'static MslPackageNode {
    use std::sync::OnceLock;
    static TREE: OnceLock<MslPackageNode> = OnceLock::new();
    TREE.get_or_init(|| {
        let mut root = MslPackageNode::new();
        for comp in crate::visual_diagram::msl_component_library() {
            // Split the qualified path into package segments + a
            // trailing class name. `Modelica.Electrical.Analog.
            // Basic.Resistor` → walk subpackages
            // [Modelica, Electrical, Analog, Basic], attach class
            // `Resistor`.
            let mut parts: Vec<&str> = comp.msl_path.split('.').collect();
            let Some(_class_name) = parts.pop() else { continue };
            let mut node = &mut root;
            for seg in parts {
                node = node
                    .subpackages
                    .entry(seg.to_string())
                    .or_insert_with(MslPackageNode::new);
            }
            node.classes.push(comp);
        }
        root
    })
}

/// Recursively render a package node as egui submenus.
///
/// Ordering per level: subpackages first (alphabetical via
/// `BTreeMap`), then a thin separator, then classes at this
/// level (own-package classes). Matches how OMEdit's library
/// browser reads: packages above, classes below.
///
/// On click of a class item we emit `AddComponent` through `out`
/// exactly as the flat menu did.
fn render_msl_package_menu(
    ui: &mut egui::Ui,
    node: &MslPackageNode,
    click_world: lunco_canvas::Pos,
    editing_class: Option<&str>,
    show_icons: bool,
    out: &mut Vec<ModelicaOp>,
) {
    for (name, child) in &node.subpackages {
        // Skip subtrees that would be entirely empty after the
        // icon-only filter. Cheap recursive walk; avoids showing
        // dead-end submenus the user can click into only to find
        // nothing.
        if !show_icons && !package_has_visible_classes(child) {
            continue;
        }
        ui.menu_button(name, |ui| {
            render_msl_package_menu(ui, child, click_world, editing_class, show_icons, out);
        });
    }
    if !node.subpackages.is_empty() && !node.classes.is_empty() {
        ui.separator();
    }
    // Sort classes alphabetically by short name for predictable
    // navigation — the library's JSON order isn't guaranteed.
    let mut classes = node.classes.clone();
    classes.sort_by(|a, b| a.name.cmp(&b.name));
    for comp in classes {
        // Hide icon-only classes unless the user explicitly enabled
        // them in Settings. Path-based detection via `is_icon_only_class`
        // (currently `.Icons.` subpackage check).
        if !show_icons && crate::class_cache::is_icon_only_class(&comp.msl_path) {
            continue;
        }
        // Display: icon character (if any) + short name. The
        // icon character gives a quick visual cue without
        // loading the SVG.
        let label = if let Some(ic) = comp.icon_text.as_deref() {
            if !ic.is_empty() {
                format!("{ic}  {}", comp.name)
            } else {
                comp.name.clone()
            }
        } else {
            comp.name.clone()
        };
        if ui
            .button(label)
            .on_hover_text(
                comp.description
                    .clone()
                    .unwrap_or_else(|| comp.msl_path.clone()),
            )
            .clicked()
        {
            if let Some(class) = editing_class {
                out.push(op_add_component(comp, click_world, class));
            }
            ui.close();
        }
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

    // Hierarchical package navigation — each submenu level mirrors
    // Modelica's package tree (Modelica → Electrical → Analog →
    // Basic → Resistor). Matches how OMEdit and Dymola present
    // the library: user drills down by package instead of
    // scanning a flat list. Tree is built once, cached.
    let show_icons = world
        .get_resource::<PaletteSettings>()
        .map(|s| s.show_icon_only_classes)
        .unwrap_or(false);
    render_msl_package_menu(
        ui,
        msl_package_tree(),
        click_world,
        editing_class,
        show_icons,
        out,
    );
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
        let active_doc = active_doc_from_world(world);
        let mut state = world.resource_mut::<CanvasDiagramState>();
        let docstate = state.get_mut(active_doc);
        if let Some(bounds) = docstate.canvas.scene.bounds() {
            let sr = lunco_canvas::Rect::from_min_max(
                lunco_canvas::Pos::new(0.0, 0.0),
                lunco_canvas::Pos::new(800.0, 600.0),
            );
            let (c, z) = docstate.canvas.viewport.fit_values(bounds, sr, 40.0);
            docstate.canvas.viewport.set_target(c, z);
        }
        ui.close();
    }
    if ui.button("⟲ Reset zoom").clicked() {
        let active_doc = active_doc_from_world(world);
        let mut state = world.resource_mut::<CanvasDiagramState>();
        let docstate = state.get_mut(active_doc);
        let c = docstate.canvas.viewport.center;
        docstate.canvas.viewport.set_target(c, 1.0);
        ui.close();
    }
}

/// Shorthand used by free helpers that don't already have the
/// active doc threaded through: resolve it from `WorkbenchState`.
/// Kept inline so callers outside the main render flow don't grow a
/// parameter just to pass a one-line lookup.
fn active_doc_from_world(world: &World) -> Option<lunco_doc::DocumentId> {
    world
        .resource::<WorkbenchState>()
        .open_model
        .as_ref()
        .and_then(|m| m.doc)
}

// ─── Drill-in loading overlay ──────────────────────────────────────

/// Rendered while a background file-read (and subsequent
/// `ReplaceSource` re-parse) is running for a drill-in target.
/// Named class, animated dots — same visual language as the
/// projection overlay but a different message so the user knows
/// it's a fresh load, not a re-project.
fn render_drill_in_loading_overlay(
    ui: &mut egui::Ui,
    canvas_rect: egui::Rect,
    class_name: &str,
    elapsed_secs: f32,
) {
    let card_w = 340.0;
    let card_h = 84.0;
    let card_rect = egui::Rect::from_center_size(
        canvas_rect.center(),
        egui::vec2(card_w, card_h),
    );
    let painter = ui.painter();
    painter.rect_filled(
        card_rect.translate(egui::vec2(0.0, 3.0)),
        8.0,
        egui::Color32::from_rgba_premultiplied(0, 0, 0, 100),
    );
    painter.rect_filled(card_rect, 8.0, egui::Color32::from_rgb(34, 38, 48));
    painter.rect_stroke(
        card_rect,
        8.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 70, 88)),
        egui::StrokeKind::Outside,
    );
    let t = ui.ctx().input(|i| i.time) as f32;
    let spinner_center = egui::pos2(card_rect.min.x + 28.0, card_rect.center().y);
    for i in 0..3 {
        let phase = (t * 2.5 - i as f32 * 0.4).rem_euclid(std::f32::consts::TAU);
        let alpha = ((phase.sin() * 0.5 + 0.5) * 255.0) as u8;
        let col = egui::Color32::from_rgba_unmultiplied(140, 200, 255, alpha);
        painter.circle_filled(
            spinner_center + egui::vec2(i as f32 * 9.0 - 9.0, 0.0),
            3.5,
            col,
        );
    }
    // Header line: "Loading resource… 12s" — the elapsed counter
    // reassures the user during slow rumoca parses (large package
    // files can take tens of seconds). Hidden in the first 0.5s to
    // avoid flicker on fast loads.
    let header = if elapsed_secs < 0.5 {
        "Loading resource…".to_string()
    } else if elapsed_secs < 10.0 {
        format!("Loading resource… {:.1}s", elapsed_secs)
    } else {
        format!("Loading resource… {}s", elapsed_secs.round() as u32)
    };
    painter.text(
        egui::pos2(card_rect.min.x + 60.0, card_rect.center().y - 8.0),
        egui::Align2::LEFT_CENTER,
        header,
        egui::FontId::proportional(13.0),
        egui::Color32::from_rgb(220, 225, 235),
    );
    // Trim long qualified names with ellipsis on the left so the
    // short class name stays visible.
    let display = if class_name.len() > 40 {
        format!("…{}", &class_name[class_name.len() - 39..])
    } else {
        class_name.to_string()
    };
    painter.text(
        egui::pos2(card_rect.min.x + 60.0, card_rect.center().y + 10.0),
        egui::Align2::LEFT_CENTER,
        display,
        egui::FontId::monospace(11.0),
        egui::Color32::from_rgb(180, 200, 225),
    );
    // Animating — request repaint so the spinner moves smoothly.
    ui.ctx().request_repaint();
}

// ─── Loading / projection overlay ──────────────────────────────────

/// Small "Projecting…" card centred on the canvas while an
/// `AsyncComputeTaskPool` projection task is in flight. Includes
/// a rotating dot so users can see the UI is responsive.
fn render_projecting_overlay(ui: &mut egui::Ui, canvas_rect: egui::Rect) {
    let card_w = 260.0;
    let card_h = 72.0;
    let card_rect = egui::Rect::from_center_size(
        canvas_rect.center(),
        egui::vec2(card_w, card_h),
    );
    let painter = ui.painter();
    painter.rect_filled(
        card_rect.translate(egui::vec2(0.0, 3.0)),
        8.0,
        egui::Color32::from_rgba_premultiplied(0, 0, 0, 90),
    );
    painter.rect_filled(card_rect, 8.0, egui::Color32::from_rgb(34, 38, 48));
    painter.rect_stroke(
        card_rect,
        8.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 70, 88)),
        egui::StrokeKind::Outside,
    );

    // Animated spinner — three dots pulsing in sequence via
    // `ctx.input(|i| i.time)`. Frame-rate independent.
    let t = ui.ctx().input(|i| i.time) as f32;
    let spinner_center = egui::pos2(card_rect.min.x + 28.0, card_rect.center().y);
    for i in 0..3 {
        let phase = (t * 2.5 - i as f32 * 0.4).rem_euclid(std::f32::consts::TAU);
        let alpha = ((phase.sin() * 0.5 + 0.5) * 255.0) as u8;
        let col = egui::Color32::from_rgba_unmultiplied(140, 200, 255, alpha);
        painter.circle_filled(
            spinner_center + egui::vec2(i as f32 * 9.0 - 9.0, 0.0),
            3.0,
            col,
        );
    }
    painter.text(
        egui::pos2(card_rect.min.x + 60.0, card_rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Loading resource…",
        egui::FontId::proportional(13.0),
        egui::Color32::from_rgb(220, 225, 235),
    );
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

    // Cheap cached fetch — rescans source only when source identity
    // changes (len + 4KB-prefix hash). Previously recompiled 5
    // regexes per frame on the 184KB source, saturating the UI
    // thread on drill-in tabs.
    let counts = empty_overlay_counts_cached(source);
    let param_count = counts.params;
    let input_count = counts.inputs;
    let output_count = counts.outputs;
    let equation_count = counts.equations;
    let connect_count = counts.connects;

    crate::ui::panels::placeholder::render_centered_card(
        ui,
        canvas_rect,
        egui::vec2(380.0, 220.0),
        |child| {
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
                // Shared placeholder layout centres each emitted
                // widget; wrap the row in a fixed-width horizontal
                // so label/value pair stays as a single centred
                // unit rather than each getting centred individually.
                u.horizontal(|u| {
                    u.label(
                        egui::RichText::new(label)
                            .small()
                            .color(egui::Color32::from_rgb(150, 160, 180)),
                    );
                    u.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |u| {
                            u.monospace(
                                egui::RichText::new(format!("{n}"))
                                    .color(egui::Color32::from_rgb(200, 220, 255)),
                            );
                        },
                    );
                });
            };
            row(child, "Parameters", param_count);
            row(child, "Inputs", input_count);
            row(child, "Outputs", output_count);
            row(child, "Equations", equation_count);
            if connect_count > 0 {
                row(child, "Connect equations", connect_count);
            }
            child.add_space(4.0);
            child.label(
                egui::RichText::new("→ Open the Text tab to read / edit the source.")
                    .italics()
                    .size(10.0)
                    .color(egui::Color32::from_rgb(140, 155, 175)),
            );
        },
    );
}

/// One-time-compiled regexes used by the empty-diagram summary.
///
/// Previously [`count_matches`] compiled a fresh `Regex` on every
/// call — fine for small user sources, catastrophic for 184 KB
/// drill-ins where the overlay fires 5× per frame at 60 Hz. Cached
/// here via `OnceLock` so compile cost is paid once at first use
/// and the per-frame work collapses to scan-only.
fn empty_overlay_regexes() -> &'static [(&'static str, regex::Regex); 5] {
    use std::sync::OnceLock;
    static RE: OnceLock<[(&str, regex::Regex); 5]> = OnceLock::new();
    RE.get_or_init(|| {
        [
            ("parameters", regex::Regex::new(r"(?m)^\s*parameter\s+").unwrap()),
            ("inputs", regex::Regex::new(r"(?m)^\s*input\s+").unwrap()),
            ("outputs", regex::Regex::new(r"(?m)^\s*output\s+").unwrap()),
            (
                "equations",
                regex::Regex::new(
                    r"(?m)^\s*(?:der\s*\(|[A-Za-z_]\w*\s*=\s*[^=])",
                )
                .unwrap(),
            ),
            (
                "connects",
                regex::Regex::new(r"\bconnect\s*\(").unwrap(),
            ),
        ]
    })
}

/// Counts for the empty-diagram overlay, cached per source so the
/// ~5 regex scans on large MSL files aren't re-run every frame.
/// Key is `(source length, blake3 hash of the first 4 KB)` — cheap
/// to compute, collision rate negligible for this use.
#[derive(Clone, Copy, Default)]
struct EmptyOverlayCounts {
    params: usize,
    inputs: usize,
    outputs: usize,
    equations: usize,
    connects: usize,
}

fn empty_overlay_counts_cached(source: &str) -> EmptyOverlayCounts {
    use std::sync::Mutex;
    use std::sync::OnceLock;
    // Source-len keyed cache is intentionally small (1 slot). The
    // overlay only shows one source at a time per active tab; if
    // two tabs alternate, worst case is we rescan once on switch.
    // Can be promoted to a HashMap keyed by DocumentId if tab
    // switching turns out to be frequent.
    static CACHE: OnceLock<Mutex<Option<(usize, u64, EmptyOverlayCounts)>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let prefix_hash = {
        let mut h: u64 = source.len() as u64;
        for b in source.as_bytes().iter().take(4096) {
            h = h.wrapping_mul(0x100000001b3).wrapping_add(*b as u64);
        }
        h
    };
    if let Ok(guard) = cache.lock() {
        if let Some((len, hash, counts)) = *guard {
            if len == source.len() && hash == prefix_hash {
                return counts;
            }
        }
    }
    let regexes = empty_overlay_regexes();
    let counts = EmptyOverlayCounts {
        params: regexes[0].1.find_iter(source).count(),
        inputs: regexes[1].1.find_iter(source).count(),
        outputs: regexes[2].1.find_iter(source).count(),
        equations: regexes[3].1.find_iter(source).count(),
        connects: regexes[4].1.find_iter(source).count(),
    };
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((source.len(), prefix_hash, counts));
    }
    counts
}

// ─── Drill-in ───────────────────────────────────────────────────────

/// Tab-to-class binding for drill-in tabs whose document hasn't
/// been installed in the registry yet. Keyed by the reserved
/// DocumentId, valued by the qualified class name the tab is
/// waiting on.
///
/// The heavy work (file read + rumoca parse) lives in
/// [`crate::class_cache::ClassCache`]; this resource only tracks
/// which tabs care about which class. When the cache resolves,
/// [`drive_drill_in_loads`] builds a `ModelicaDocument` from the
/// cached AST + source (no second parse) and installs it into the
/// registry, clearing the binding.
///
/// The name `DrillInLoads` is preserved for minimal churn; the
/// resource is effectively "tabs waiting on a class cache entry".
#[derive(bevy::prelude::Resource, Default)]
pub struct DrillInLoads {
    pending: std::collections::HashMap<lunco_doc::DocumentId, DrillInBinding>,
}

/// Persistent `DocumentId → qualified class name` map for tabs
/// opened via drill-in. Lives for the tab's lifetime (cleared by
/// [`cleanup_removed_documents`]), so downstream systems — canvas
/// projection, especially — can ask "what class was this tab
/// drilled into?" after install has already cleared the transient
/// [`DrillInLoads`] entry.
///
/// Without this, projection for a drill-in tab can't scope to the
/// specific class: the installed `ModelicaDocument.canonical_path`
/// is the `.mo` file, which for multi-class package files doesn't
/// tell us which of the dozen classes inside the user meant.
#[derive(bevy::prelude::Resource, Default)]
pub struct DrilledInClassNames {
    pub by_doc: std::collections::HashMap<lunco_doc::DocumentId, String>,
}

impl DrilledInClassNames {
    pub fn get(&self, doc: lunco_doc::DocumentId) -> Option<&str> {
        self.by_doc.get(&doc).map(String::as_str)
    }
    pub fn set(&mut self, doc: lunco_doc::DocumentId, qualified: String) {
        self.by_doc.insert(doc, qualified);
    }
    pub fn remove(&mut self, doc: lunco_doc::DocumentId) -> Option<String> {
        self.by_doc.remove(&doc)
    }
}

pub struct DrillInBinding {
    pub qualified: String,
    /// When the tab was opened. Used to show elapsed-seconds in the
    /// loading overlay so the user sees work is happening even when
    /// rumoca takes tens of seconds on large package files.
    pub started: std::time::Instant,
}

impl DrillInLoads {
    pub fn is_loading(&self, doc: lunco_doc::DocumentId) -> bool {
        self.pending.contains_key(&doc)
    }
    pub fn detail(&self, doc: lunco_doc::DocumentId) -> Option<&str> {
        self.pending.get(&doc).map(|b| b.qualified.as_str())
    }
    /// `(qualified, seconds elapsed since tab opened)` for the
    /// loading overlay. Returns `None` if nothing is loading for
    /// this doc.
    pub fn progress(&self, doc: lunco_doc::DocumentId) -> Option<(&str, f32)> {
        self.pending
            .get(&doc)
            .map(|b| (b.qualified.as_str(), b.started.elapsed().as_secs_f32()))
    }
}

/// Bevy system: for each pending drill-in binding, check whether
/// its class has landed in [`ClassCache`]. If yes, build a
/// `ModelicaDocument` from the cached parts (no re-parse) and
/// install it in the registry.
pub fn drive_drill_in_loads(
    mut loads: bevy::prelude::ResMut<DrillInLoads>,
    mut registry: bevy::prelude::ResMut<ModelicaDocumentRegistry>,
    cache: Option<bevy::prelude::Res<crate::class_cache::ClassCache>>,
    mut tabs: bevy::prelude::ResMut<crate::ui::panels::model_view::ModelTabs>,
    mut class_names: bevy::prelude::ResMut<DrilledInClassNames>,
) {
    use bevy::prelude::*;
    let Some(cache) = cache else { return };
    // Snapshot pairs first so we can mutate `pending` in the loop.
    let pending: Vec<(lunco_doc::DocumentId, String)> = loads
        .pending
        .iter()
        .map(|(k, v)| (*k, v.qualified.clone()))
        .collect();
    for (doc_id, qualified) in pending {
        // Still loading?
        if cache.is_loading(&qualified) {
            continue;
        }
        // Ready?
        if let Some(entry) = cache.peek(&qualified) {
            let origin = lunco_doc::DocumentOrigin::File {
                path: entry.file_path.clone(),
                writable: false,
            };
            // Build doc from pre-parsed cache entry — zero rumoca work.
            let doc = crate::document::ModelicaDocument::from_parts(
                doc_id,
                entry.source.to_string(),
                origin,
                std::sync::Arc::clone(&entry.ast),
            );
            registry.install_prebuilt(doc_id, doc);
            loads.pending.remove(&doc_id);
            // Persistent binding so projection can scope to this
            // class even after `loads` is cleared. Required for
            // multi-class package files where
            // `ModelicaDocument.canonical_path` only tells us the
            // `.mo` file, not which class inside the user meant.
            class_names.set(doc_id, qualified.clone());
            // Smart default view for the drilled-in tab. Matches
            // OMEdit/Dymola's "all three views always visible, but
            // land in the one the user probably wants" behaviour:
            //
            //   - Icon-only class (`.Icons.*` subtree) → Icon. No
            //     connectors, no diagram content ever.
            //   - Class with zero instantiated components (primitive
            //     `block` like `CriticalDamping`, `partial` templates,
            //     pure-equation `model`s) → Icon. The Diagram layer
            //     is legitimately empty; Icon shows the visual
            //     symbol the user expects.
            //   - Composed model with components → Canvas (stay on
            //     the default). Drill-in was the user asking "what's
            //     inside?" and there's something to show.
            //
            // `find_class_by_qualified_name` walks the (already
            // parsed) AST from the cache entry — no extra parsing.
            let has_components = entry.ast.ast().and_then(|ast| {
                crate::diagram::find_class_by_qualified_name(ast, &qualified)
                    .map(|c| !c.components.is_empty())
            });
            let land_in_icon_view =
                crate::class_cache::is_icon_only_class(&qualified)
                    || has_components == Some(false);
            if land_in_icon_view {
                if let Some(tab) = tabs.get_mut(doc_id) {
                    tab.view_mode =
                        crate::ui::panels::model_view::ModelViewMode::Icon;
                }
            }
            info!(
                "[CanvasDiagram] drill-in: installed `{}` from `{}` (cache hit)",
                qualified,
                entry.file_path.display()
            );
            continue;
        }
        // Failed — log once and drop the binding. Tab will show
        // the empty-diagram overlay; user can close it.
        if let Some(msg) = cache.failure_message(&qualified) {
            warn!(
                "[CanvasDiagram] drill-in: class `{}` load failed: {}",
                qualified, msg
            );
            loads.pending.remove(&doc_id);
        }
    }
}

/// Open the Modelica class with `qualified` name in a new tab.
/// The tab appears immediately with an empty document showing a
/// "Loading…" overlay; the file read happens on a background task
/// and the source is applied via `ReplaceSource` when the read
/// completes. This matches what users expect: the tab opens, a
/// spinner says "loading", content lands when it's ready.
fn drill_into_class(world: &mut World, qualified: &str) {
    if !qualified.starts_with("Modelica.") {
        bevy::log::info!(
            "[CanvasDiagram] drill-in skipped — `{}` is not an MSL class (user classes TBD)",
            qualified
        );
        return;
    }
    let Some(file_path) = crate::class_cache::resolve_msl_class_path(qualified) else {
        bevy::log::warn!(
            "[CanvasDiagram] drill-in: could not locate MSL file for `{}`",
            qualified
        );
        return;
    };
    open_drill_in_tab(world, qualified, &file_path);
}

/// Open a tab for `qualified` class backed by a **placeholder
/// document** — empty source, parses instantly. Spawns a bg task
/// that reads the file; a later Bevy system applies `ReplaceSource`
/// when the read completes.
///
/// The user sees:
///  1. Instant: a new tab titled with the class short name.
///  2. Immediately: an "Loading…" overlay on the canvas.
///  3. A moment later: the real source + diagram populates.
///
/// If a tab for the same file path is already open (from a
/// previous drill-in), we focus it instead of making a second.
fn open_drill_in_tab(
    world: &mut World,
    qualified: &str,
    file_path: &std::path::Path,
) {
    // Find or allocate the doc. Reuse an existing one if the same
    // msl:// path was opened before, so re-drilling into the same
    // class focuses instead of spawning a duplicate.
    let model_path_id = format!("msl://{qualified}");
    let existing_doc = {
        let registry = world.resource::<ModelicaDocumentRegistry>();
        // ModelicaDocumentRegistry doesn't expose a find-by-path
        // API, so we look through existing tabs for a match.
        let tabs = world.resource::<crate::ui::panels::model_view::ModelTabs>();
        tabs.iter_docs().find(|&doc_id| {
            registry
                .host(doc_id)
                .and_then(|h| match h.document().origin() {
                    lunco_doc::DocumentOrigin::File { path, .. } => {
                        Some(path == file_path)
                    }
                    _ => None,
                })
                .unwrap_or(false)
        })
    };
    let (doc_id, needs_load) = if let Some(id) = existing_doc {
        (id, false)
    } else {
        // Reserve a doc id only; the actual `ModelicaDocument`
        // (including the rumoca parse) is built on a background
        // thread and installed via `install_prebuilt` when ready.
        // Queries against the id before install return `None` —
        // panels render the "Loading resource…" overlay based on
        // `DrillInLoads::is_loading`.
        let mut registry = world.resource_mut::<ModelicaDocumentRegistry>();
        let id = registry.reserve_id();
        (id, true)
    };

    if needs_load {
        // Kick (or piggyback on) a class cache load. If AddComponent
        // preloaded this class earlier, the entry is already cached
        // and the tab installs on the very next `drive` tick — no
        // file read, no parse. Concurrent tabs opening the same
        // class dedupe onto one load automatically.
        crate::class_cache::request_class(world, qualified);
        let mut loads = world.resource_mut::<DrillInLoads>();
        loads.pending.insert(
            doc_id,
            DrillInBinding {
                qualified: qualified.to_string(),
                started: std::time::Instant::now(),
            },
        );
    }

    let _ = model_path_id;

    // Register the tab + land the user in Canvas view (they
    // drilled FROM a canvas, so the canvas is what they expect
    // to see). Default `view_mode` is Text for newly-created
    // scratch models; drill-in is a different use case.
    {
        let mut model_tabs =
            world.resource_mut::<crate::ui::panels::model_view::ModelTabs>();
        model_tabs.ensure(doc_id);
        if let Some(tab) = model_tabs.get_mut(doc_id) {
            tab.view_mode = crate::ui::panels::model_view::ModelViewMode::Canvas;
        }
    }
    world.commands().trigger(lunco_workbench::OpenTab {
        kind: crate::ui::panels::model_view::MODEL_VIEW_KIND,
        instance: doc_id.raw(),
    });

    bevy::log::info!(
        "[CanvasDiagram] drill-in: opened placeholder tab for `{}` (file: `{}`) — loading in background",
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
    let active_doc = active_doc_from_world(world);
    let state = world.resource::<CanvasDiagramState>();
    let scene = &state.get(active_doc).canvas.scene;
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
    let active_doc = active_doc_from_world(world);
    let state = world.resource::<CanvasDiagramState>();
    let Some(node) = state.get(active_doc).canvas.scene.node(id) else {
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
    let active_doc = active_doc_from_world(world);
    let state = world.resource::<CanvasDiagramState>();
    op_remove_node_inner(&state.get(active_doc).canvas.scene, id, class)
}

fn op_remove_edge(
    world: &mut World,
    id: lunco_canvas::EdgeId,
    class: &str,
) -> Option<ModelicaOp> {
    let active_doc = active_doc_from_world(world);
    let state = world.resource::<CanvasDiagramState>();
    op_remove_edge_inner(&state.get(active_doc).canvas.scene, id, class)
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
    // Read-only guard — only block ops when the user is viewing a
    // TRULY read-only tab (MSL / bundled library class), NOT when
    // the doc is merely Untitled. `Document::is_read_only` in this
    // codebase means "can't save-to-disk without Save-As", which is
    // true for Untitled despite Untitled being fully editable. The
    // Package Browser / drill-in sets
    // `WorkbenchState.open_model.read_only` to `true` only for
    // library classes, so gate on that instead.
    let is_read_only = world
        .get_resource::<WorkbenchState>()
        .and_then(|s| s.open_model.as_ref())
        .map(|m| m.read_only)
        .unwrap_or(false);
    if is_read_only {
        bevy::log::info!(
            "[CanvasDiagram] discarded {} op(s) — tab is a read-only library class",
            n
        );
        return;
    }

    // Preload any class the user just referenced. Fire-and-forget —
    // the two-tier cache (FileCache + ClassCache) dedupes by file,
    // so adding ten Resistors triggers at most one parse of the
    // Resistor.mo file across this session.
    for op in &ops {
        if let ModelicaOp::AddComponent { decl, .. } = op {
            if decl.type_name.starts_with("Modelica.") {
                crate::class_cache::request_class(world, &decl.type_name);
            }
        }
    }

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
