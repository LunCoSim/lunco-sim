//! Diagram panel — Dymola-style visual canvas for building Modelica models.
//!
//! ## Architecture
//!
//! The diagram panel owns a **VisualDiagram** (the data model) and renders it
//! through `egui-snarl` with heavily customised `SnarlViewer` overrides:
//!
//! - **Custom component shapes** drawn in `show_body()` — zigzag resistors,
//!   parallel-plate capacitors, inductor loops, ground symbols, voltage sources.
//! - **Coloured port dots** in `show_input()`/`show_output()` — small circles
//!   colour-coded by connector domain (electrical = blue, mechanical = green,
//!   signal = orange).
//! - **Borderless node frames** via `node_frame()` — dark translucent cards
//!   with thin borders for a clean schematic look.
//! - **Dot-grid background** via `draw_background()` — subtle dots at 20 px
//!   spacing for a professional draughting feel.
//! - **Right-click context menus** via `show_graph_menu()` / `show_node_menu()`
//!   — add components from anywhere, delete/edit nodes.
//!
//! ## Rendering Modes
//!
//! A mode toggle button on the toolbar switches between:
//! - **Schematic** — Dymola-style custom shapes (default).
//! - **NodeGraph** — generic snarl rectangles with labels (debugging fallback).

use bevy::prelude::*;
use bevy::tasks::Task;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};
use egui_snarl::{InPin, InPinId, OutPin, OutPinId, NodeId, Snarl};
use egui_snarl::ui::{SnarlViewer, SnarlPin, PinInfo, SnarlStyle, BackgroundPattern};
use std::collections::HashMap;

use crate::visual_diagram::{
    DiagramNodeId, VisualDiagram, MSLComponentDef,
    generate_modelica_source, msl_component_library,
    msl_categories, msl_components_in_category,
};
use crate::ui::WorkbenchState;
use crate::{ModelicaModel, ModelicaChannels, ModelicaCommand};
use lunco_doc::DocumentId;

// ---------------------------------------------------------------------------
// Design Tokens — all visual constants live here (Tunability Mandate).
// ---------------------------------------------------------------------------

/// Tunable design tokens for the diagram rendering.
///
/// Per Article X of the Project Constitution, hardcoded magic numbers are
/// forbidden. All visual parameters are collected in this resource so they
/// can be adjusted at runtime or from a theme file.
#[derive(Resource, Clone)]
pub struct DiagramTheme {
    // ── Background ──
    /// Spacing between dot-grid points (pixels).
    pub grid_spacing: f32,
    /// Radius of each grid dot (pixels).
    pub grid_dot_radius: f32,
    /// Colour of the grid dots.
    pub grid_dot_color: egui::Color32,

    // ── Node frame ──
    /// Node card background colour.
    pub node_bg: egui::Color32,
    /// Node card border stroke.
    pub node_stroke: egui::Stroke,
    /// Node card corner rounding (u8 per egui 0.34 `CornerRadius::same`).
    pub node_rounding: u8,

    // ── Schematic body ──
    /// Line width for component symbols.
    pub symbol_stroke_width: f32,
    /// Default symbol colour.
    pub symbol_color: egui::Color32,
    /// Body minimum size for the schematic drawing area.
    pub body_min_size: egui::Vec2,

    // ── Port dots ──
    /// Radius of the port indicator dots.
    pub port_dot_radius: f32,
    /// Colour for electrical (Pin) connectors.
    pub color_electrical: egui::Color32,
    /// Colour for mechanical (Flange) connectors.
    pub color_mechanical: egui::Color32,
    /// Colour for signal (RealInput/RealOutput) connectors.
    pub color_signal: egui::Color32,
    /// Colour for unknown/generic connectors.
    pub color_generic: egui::Color32,
}

impl Default for DiagramTheme {
    fn default() -> Self {
        Self {
            grid_spacing: 15.0,
            grid_dot_radius: 0.8,
            grid_dot_color: egui::Color32::from_gray(40),

            node_bg: egui::Color32::from_rgba_premultiplied(15, 15, 20, 230),
            node_stroke: egui::Stroke::new(1.0, egui::Color32::from_rgb(130, 130, 140)),
            node_rounding: 0,

            symbol_stroke_width: 2.0,
            symbol_color: egui::Color32::from_rgb(220, 220, 230),
            body_min_size: egui::Vec2::new(80.0, 80.0),

            port_dot_radius: 4.0,
            color_electrical: egui::Color32::from_rgb(70, 140, 255),
            color_mechanical: egui::Color32::from_rgb(80, 200, 120),
            color_signal: egui::Color32::from_rgb(230, 160, 50),
            color_generic: egui::Color32::from_rgb(180, 180, 180),
        }
    }
}

// ---------------------------------------------------------------------------
// Auto-layout settings
// ---------------------------------------------------------------------------

/// Grid-layout parameters used when an imported model has no authored
/// `annotation(Placement(...))` on its components. Tunable per the
/// Article-X mandate — no magic numbers buried inside the import path.
///
/// Two independent grids because the two importers have different
/// failure modes: the AST importer runs on valid-enough source with
/// known icon extents (tighter grid reads well), while the regex
/// scan is a last-resort recovery from broken source and leaves
/// extra breathing room so overlapping labels stay legible.
#[derive(Resource, Clone, Debug)]
pub struct DiagramAutoLayoutSettings {
    /// Grid spacing (world units) between columns for components
    /// without a `Placement` annotation. Slot is keyed by the node's
    /// index in the class's component list — stable under sibling
    /// annotation changes, so dragging one component doesn't shift
    /// the others.
    pub spacing_x: f32,
    /// Grid spacing between rows.
    pub spacing_y: f32,
    /// Column count; nodes wrap to a new row once reached.
    pub cols: usize,
    /// Fraction of `spacing_x` to offset odd rows by — stagger keeps
    /// ports on the shared horizontal band from wiring through the
    /// icon body of the row above.
    pub row_stagger: f32,
}

impl Default for DiagramAutoLayoutSettings {
    fn default() -> Self {
        Self {
            spacing_x: 140.0,
            spacing_y: 110.0,
            cols: 4,
            row_stagger: 0.5,
        }
    }
}

// ---------------------------------------------------------------------------
// Diagram State
// ---------------------------------------------------------------------------

/// Resource holding the visual diagram being built on the canvas.
///
/// # Phase α migration
///
/// The panel is transitioning from "the `VisualDiagram` + `snarl` fields
/// are the authoring truth, source is regenerated on compile" to "the
/// active `ModelicaDocument` is the authoring truth, snarl is a rendered
/// projection of the document's AST". During the transition both
/// authoring paths coexist:
///
/// - User actions (drag from palette, wire ports, drag-to-move) emit
///   AST-level ops to [`DiagramState::document`]. The
///   [`DiagramState::last_seen_gen`] cursor tracks which of the
///   document's structured changes have already been applied to
///   `snarl`.
/// - The legacy [`DiagramState::diagram`] and `generate_modelica_source`
///   path still runs on compile so existing behaviour is preserved until
///   the ops-driven compile lands. Both will be retired once snarl
///   renders directly from the document AST.
#[derive(Resource)]
pub struct DiagramState {
    /// The active Modelica document the diagram is editing. Resolved
    /// from `WorkbenchState.open_model.doc` when a document is open,
    /// lazily allocated otherwise (see
    /// [`DiagramState::ensure_document`]).
    pub document: Option<DocumentId>,
    /// Last document generation the snarl render cache has consumed
    /// from [`ModelicaDocument::changes_since`]. Drives incremental
    /// snarl patching — see Phase α design.
    pub last_seen_gen: u64,
    /// The visual model being built (legacy authoring store —
    /// scheduled for removal once all ops route through
    /// [`DiagramState::document`]).
    pub diagram: VisualDiagram,
    /// The egui-snarl state for the canvas.
    pub snarl: Snarl<DiagramNode>,
    /// Generated source from last compile.
    /// Compile status message.
    pub compile_status: Option<String>,
    /// Whether last compile succeeded.
    pub compile_ok: bool,
    /// Counter for model names.
    pub model_counter: u32,
    /// Counter for component placement positions.
    pub placement_counter: u32,
    /// Active background parsing task.
    pub parse_task: Option<Task<Option<VisualDiagram>>>,
    /// Whether schematic mode is enabled (true) vs. generic node-graph mode (false).
    pub schematic_mode: bool,
    /// Persistent storage for the last graph-space position where a menu was triggered.
    pub last_click_pos: egui::Pos2,
    /// The currently selected node for inspection.
    pub selected_node: Option<DiagramNodeId>,
    /// Snapshot of the wire set at the end of the previous frame.
    /// Each entry is an unordered pair of `(component, port)` tuples.
    /// Used to detect newly-drawn / newly-removed wires in snarl
    /// between frames, so the corresponding `AddConnection` /
    /// `RemoveConnection` ops can be emitted to the document.
    pub last_wires: std::collections::HashSet<((String, String), (String, String))>,
    /// Snapshot of node positions (keyed by Modelica instance name)
    /// at the end of the previous frame. Values are in Modelica
    /// diagram coordinates (+Y up). Used to detect drag-to-move
    /// between frames so `SetPlacement` ops can be emitted.
    ///
    /// `None` marks the post-bind initial state where no positions
    /// have been recorded yet — the next frame's positions become
    /// the baseline and don't emit any ops.
    pub last_positions: Option<std::collections::HashMap<String, (f32, f32)>>,
}

impl DiagramState {
    /// Add a component to both the diagram data and the snarl UI.
    pub fn add_component(&mut self, def: MSLComponentDef, pos: egui::Pos2) {
        let node_id = self.diagram.add_node(def.clone(), pos);
        let ports: Vec<String> = def.ports.iter().map(|p| p.name.clone()).collect();
        let connector_types: Vec<String> = def.ports.iter().map(|p| p.connector_type.clone()).collect();
        let port_positions: Vec<(f32, f32)> = def.ports.iter().map(|p| (p.x, p.y)).collect();

        let snarl_node = DiagramNode::Component {
            id: node_id,
            instance_name: self.diagram.get_node(node_id).unwrap().instance_name.clone(),
            type_name: def.name.clone(),
            description: def.description.clone(),
            icon_text: def.icon_text.clone(),
            icon_asset: def.icon_asset.clone(),
            ports,
            connector_types,
            port_positions,
        };

        self.snarl.insert_node(pos, snarl_node);
    }

    /// Rebuild the snarl from the current diagram.
    pub fn rebuild_snarl(&mut self) {
        self.snarl = build_snarl(&self.diagram);
    }

    /// Bind this diagram panel to a particular Modelica document,
    /// resetting the change-stream cursor so the next sync does a
    /// clean rebuild of the snarl from the document's current AST.
    ///
    /// Called when the user switches `open_model` or on first access
    /// to a freshly-allocated document.
    pub fn bind_document(&mut self, doc: DocumentId) {
        if self.document != Some(doc) {
            self.document = Some(doc);
            self.last_seen_gen = 0;
            self.last_wires.clear();
            self.last_positions = None;
        }
    }

    /// Detach from any currently-bound document. Used when the user
    /// closes the active model. Leaves snarl / diagram state intact so
    /// the canvas doesn't visibly flash empty during transitions.
    pub fn unbind_document(&mut self) {
        self.document = None;
        self.last_seen_gen = 0;
        self.last_wires.clear();
        self.last_positions = None;
    }
}

impl Default for DiagramState {
    fn default() -> Self {
        Self {
            document: None,
            last_seen_gen: 0,
            diagram: VisualDiagram::default(),
            snarl: Snarl::default(),
            compile_status: None,
            compile_ok: false,
            model_counter: 0,
            placement_counter: 0,
            parse_task: None,
            schematic_mode: true,
            last_click_pos: egui::Pos2::ZERO,
            selected_node: None,
            last_wires: std::collections::HashSet::new(),
            last_positions: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Snarl Node Type
// ---------------------------------------------------------------------------

/// A visual node on the diagram canvas.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum DiagramNode {
    Component {
        id: DiagramNodeId,
        instance_name: String,
        type_name: String,
        description: Option<String>,
        icon_text: Option<String>,
        icon_asset: Option<String>,
        ports: Vec<String>,
        connector_types: Vec<String>,
        /// Port positions in Modelica diagram coordinates (-100..100).
        /// Parallel to `ports` and `connector_types`.
        #[serde(default)]
        port_positions: Vec<(f32, f32)>,
    },
}

impl DiagramNode {
    fn from_msl(comp: &MSLComponentDef) -> Self {
        let ports: Vec<String> = comp.ports.iter().map(|p| p.name.clone()).collect();
        let connector_types: Vec<String> = comp.ports.iter().map(|p| p.connector_type.clone()).collect();
        let port_positions: Vec<(f32, f32)> = comp.ports.iter().map(|p| (p.x, p.y)).collect();
        DiagramNode::Component {
            id: DiagramNodeId::new(),
            instance_name: format!("New{}", comp.name),
            type_name: comp.name.clone(),
            description: comp.description.clone(),
            icon_text: comp.icon_text.clone(),
            icon_asset: comp.icon_asset.clone(),
            ports,
            connector_types,
            port_positions,
        }
    }

    fn title(&self) -> &str {
        match self {
            DiagramNode::Component { instance_name, .. } => instance_name,
        }
    }

    fn subtitle(&self) -> &str {
        match self {
            DiagramNode::Component { type_name, .. } => type_name,
        }
    }

    fn port_count(&self) -> usize {
        match self {
            DiagramNode::Component { ports, .. } => ports.len(),
        }
    }

    fn connector_type_at(&self, idx: usize) -> Option<&str> {
        match self {
            DiagramNode::Component { connector_types, .. } => {
                connector_types.get(idx).map(|s| s.as_str())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Schematic Symbol Drawing
// ---------------------------------------------------------------------------

/// Draw a resistor zigzag symbol inside the given rectangle.
fn draw_resistor(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.symbol_color);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let rw = rect.width() * 0.5;
    let rh = rect.height() * 0.22;

    // Sharper standard zigzag (3 peaks)
    let left = cx - rw/2.0;
    let dx = rw / 6.0;
    let points = vec![
        egui::Pos2::new(left, cy),
        egui::Pos2::new(left + dx/2.0, cy - rh),
        egui::Pos2::new(left + dx * 1.5, cy + rh),
        egui::Pos2::new(left + dx * 2.5, cy - rh),
        egui::Pos2::new(left + dx * 3.5, cy + rh),
        egui::Pos2::new(left + dx * 4.5, cy - rh),
        egui::Pos2::new(left + dx * 5.5, cy + rh),
        egui::Pos2::new(left + rw, cy),
    ];

    for window in points.windows(2) {
        painter.line_segment([window[0], window[1]], stroke);
    }

    // Lead wires
    painter.line_segment([egui::Pos2::new(rect.left() + 2.0, cy), egui::Pos2::new(left, cy)], stroke);
    painter.line_segment([egui::Pos2::new(rect.right() - 2.0, cy), egui::Pos2::new(left + rw, cy)], stroke);
}

/// Draw a capacitor symbol (two parallel plates) inside the given rectangle.
fn draw_capacitor(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.symbol_color);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let plate_h = rect.height() * 0.6;
    let gap = rect.width() * 0.08;

    // Left wire
    painter.line_segment(
        [egui::Pos2::new(rect.left() + 2.0, cy), egui::Pos2::new(cx - gap, cy)],
        stroke,
    );
    // Left plate
    painter.line_segment(
        [egui::Pos2::new(cx - gap, cy - plate_h / 2.0), egui::Pos2::new(cx - gap, cy + plate_h / 2.0)],
        egui::Stroke::new(theme.symbol_stroke_width + 0.5, theme.symbol_color),
    );
    // Right plate
    painter.line_segment(
        [egui::Pos2::new(cx + gap, cy - plate_h / 2.0), egui::Pos2::new(cx + gap, cy + plate_h / 2.0)],
        egui::Stroke::new(theme.symbol_stroke_width + 0.5, theme.symbol_color),
    );
    // Right wire
    painter.line_segment(
        [egui::Pos2::new(cx + gap, cy), egui::Pos2::new(rect.right() - 2.0, cy)],
        stroke,
    );
}

/// Draw an inductor symbol (semicircular loops) inside the given rectangle.
fn draw_inductor(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.symbol_color);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let loops = 3;
    let loop_w = rect.width() * 0.6 / loops as f32;
    let loop_r = loop_w / 2.0;
    let total_w = loop_w * loops as f32;

    // Left wire
    painter.line_segment(
        [egui::Pos2::new(rect.left() + 4.0, cy), egui::Pos2::new(cx - total_w / 2.0, cy)],
        stroke,
    );

    // Semicircular bumps (approximated with arcs)
    for i in 0..loops {
        let arc_cx = cx - total_w / 2.0 + loop_r + i as f32 * loop_w;
        let n_segs = 12;
        let mut arc_pts = Vec::with_capacity(n_segs + 1);
        for s in 0..=n_segs {
            let angle = std::f32::consts::PI * s as f32 / n_segs as f32;
            let x = arc_cx + loop_r * angle.cos();
            let y = cy - loop_r * angle.sin();
            arc_pts.push(egui::Pos2::new(x, y));
        }
        for pair in arc_pts.windows(2) {
            painter.line_segment([pair[0], pair[1]], stroke);
        }
    }

    // Right wire
    painter.line_segment(
        [egui::Pos2::new(cx + total_w / 2.0, cy), egui::Pos2::new(rect.right() - 4.0, cy)],
        stroke,
    );
}

/// Draw a ground symbol (three horizontal lines decreasing in width).
fn draw_ground(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.symbol_color);
    let cx = rect.center().x;
    let top_y = rect.top() + rect.height() * 0.15;
    let bottom_y = rect.bottom() - rect.height() * 0.1;
    let bar_spacing = (bottom_y - top_y) / 4.0;

    // Vertical wire from top
    painter.line_segment(
        [egui::Pos2::new(cx, top_y), egui::Pos2::new(cx, top_y + bar_spacing)],
        stroke,
    );

    // Three horizontal bars (widest to narrowest)
    let widths = [0.7, 0.45, 0.2];
    for (i, &w_frac) in widths.iter().enumerate() {
        let y = top_y + bar_spacing * (i as f32 + 1.0);
        let half_w = rect.width() * w_frac / 2.0;
        painter.line_segment(
            [egui::Pos2::new(cx - half_w, y), egui::Pos2::new(cx + half_w, y)],
            stroke,
        );
    }
}

/// Draw a voltage source symbol (circle with +/−).
fn draw_voltage_source(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.symbol_color);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let radius = rect.height().min(rect.width()) * 0.32;

    // Circle
    painter.circle_stroke(egui::Pos2::new(cx, cy), radius, stroke);

    // + sign (upper half)
    let plus_y = cy - radius * 0.4;
    let sign_len = radius * 0.25;
    painter.line_segment(
        [egui::Pos2::new(cx - sign_len, plus_y), egui::Pos2::new(cx + sign_len, plus_y)],
        stroke,
    );
    painter.line_segment(
        [egui::Pos2::new(cx, plus_y - sign_len), egui::Pos2::new(cx, plus_y + sign_len)],
        stroke,
    );

    // − sign (lower half)
    let minus_y = cy + radius * 0.4;
    painter.line_segment(
        [egui::Pos2::new(cx - sign_len, minus_y), egui::Pos2::new(cx + sign_len, minus_y)],
        stroke,
    );

    // Lead wires
    painter.line_segment(
        [egui::Pos2::new(rect.left() + 4.0, cy), egui::Pos2::new(cx - radius, cy)],
        stroke,
    );
    painter.line_segment(
        [egui::Pos2::new(cx + radius, cy), egui::Pos2::new(rect.right() - 4.0, cy)],
        stroke,
    );
}

/// Draw a current source symbol (circle with arrow).
fn draw_current_source(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.symbol_color);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let radius = rect.height().min(rect.width()) * 0.32;

    // Circle
    painter.circle_stroke(egui::Pos2::new(cx, cy), radius, stroke);

    // Arrow pointing up
    let arrow_len = radius * 0.6;
    painter.line_segment(
        [egui::Pos2::new(cx, cy + arrow_len / 2.0), egui::Pos2::new(cx, cy - arrow_len / 2.0)],
        stroke,
    );
    // Arrowhead
    let ah = radius * 0.2;
    painter.line_segment(
        [egui::Pos2::new(cx - ah, cy - arrow_len / 2.0 + ah), egui::Pos2::new(cx, cy - arrow_len / 2.0)],
        stroke,
    );
    painter.line_segment(
        [egui::Pos2::new(cx + ah, cy - arrow_len / 2.0 + ah), egui::Pos2::new(cx, cy - arrow_len / 2.0)],
        stroke,
    );

    // Lead wires
    painter.line_segment(
        [egui::Pos2::new(rect.left() + 4.0, cy), egui::Pos2::new(cx - radius, cy)],
        stroke,
    );
    painter.line_segment(
        [egui::Pos2::new(cx + radius, cy), egui::Pos2::new(rect.right() - 4.0, cy)],
        stroke,
    );
}

/// Draw a sensor symbol (circle with a diagonal arrow — meter style).
fn draw_sensor(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.symbol_color);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let radius = rect.height().min(rect.width()) * 0.30;

    // Circle (dashed feel via thinner stroke)
    painter.circle_stroke(
        egui::Pos2::new(cx, cy),
        radius,
        egui::Stroke::new(theme.symbol_stroke_width * 0.8, theme.color_signal),
    );

    // Diagonal needle
    let needle_len = radius * 0.7;
    let angle = -std::f32::consts::FRAC_PI_4;
    painter.line_segment(
        [
            egui::Pos2::new(cx, cy),
            egui::Pos2::new(cx + needle_len * angle.cos(), cy + needle_len * angle.sin()),
        ],
        stroke,
    );

    // Lead wires
    painter.line_segment(
        [egui::Pos2::new(rect.left() + 4.0, cy), egui::Pos2::new(cx - radius, cy)],
        stroke,
    );
    painter.line_segment(
        [egui::Pos2::new(cx + radius, cy), egui::Pos2::new(rect.right() - 4.0, cy)],
        stroke,
    );
}

/// Draw a switch symbol (open contact).
fn draw_switch(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.symbol_color);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let half_w = rect.width() * 0.3;

    // Left wire
    painter.line_segment(
        [egui::Pos2::new(rect.left() + 4.0, cy), egui::Pos2::new(cx - half_w, cy)],
        stroke,
    );
    // Right wire
    painter.line_segment(
        [egui::Pos2::new(cx + half_w, cy), egui::Pos2::new(rect.right() - 4.0, cy)],
        stroke,
    );
    // Left contact (dot)
    painter.circle_filled(egui::Pos2::new(cx - half_w, cy), 3.0, theme.symbol_color);
    // Right contact (dot)
    painter.circle_filled(egui::Pos2::new(cx + half_w, cy), 3.0, theme.symbol_color);
    // Open lever
    painter.line_segment(
        [
            egui::Pos2::new(cx - half_w, cy),
            egui::Pos2::new(cx + half_w * 0.6, cy - rect.height() * 0.35),
        ],
        stroke,
    );
}

/// Draw a spring symbol (translational mechanics).
fn draw_spring(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.color_mechanical);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let w = rect.width() * 0.7;
    let h = rect.height() * 0.3;
    let coils = 4;
    let seg_w = w / coils as f32;
    let half_w = w / 2.0;

    // Left wire
    painter.line_segment(
        [egui::Pos2::new(rect.left() + 4.0, cy), egui::Pos2::new(cx - half_w, cy)],
        stroke,
    );

    // Zigzag coil
    let mut pts = Vec::with_capacity(coils * 2 + 2);
    pts.push(egui::Pos2::new(cx - half_w, cy));
    for i in 0..coils {
        let base_x = cx - half_w + i as f32 * seg_w;
        pts.push(egui::Pos2::new(base_x + seg_w * 0.25, cy - h));
        pts.push(egui::Pos2::new(base_x + seg_w * 0.75, cy + h));
    }
    pts.push(egui::Pos2::new(cx + half_w, cy));
    for pair in pts.windows(2) {
        painter.line_segment([pair[0], pair[1]], stroke);
    }

    // Right wire
    painter.line_segment(
        [egui::Pos2::new(cx + half_w, cy), egui::Pos2::new(rect.right() - 4.0, cy)],
        stroke,
    );
}

/// Draw a damper symbol (piston).
fn draw_damper(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.color_mechanical);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let box_w = rect.width() * 0.3;
    let box_h = rect.height() * 0.5;

    // Left wire
    painter.line_segment(
        [egui::Pos2::new(rect.left() + 4.0, cy), egui::Pos2::new(cx - box_w, cy)],
        stroke,
    );

    // Piston rod going into cylinder
    painter.line_segment(
        [egui::Pos2::new(cx - box_w, cy), egui::Pos2::new(cx, cy)],
        stroke,
    );

    // Piston head
    painter.line_segment(
        [egui::Pos2::new(cx, cy - box_h / 2.0), egui::Pos2::new(cx, cy + box_h / 2.0)],
        egui::Stroke::new(theme.symbol_stroke_width + 1.0, theme.color_mechanical),
    );

    // Cylinder (open rectangle around the piston)
    let cyl_left = cx - box_w * 0.1;
    painter.line_segment(
        [egui::Pos2::new(cyl_left, cy - box_h / 2.0), egui::Pos2::new(cx + box_w, cy - box_h / 2.0)],
        stroke,
    );
    painter.line_segment(
        [egui::Pos2::new(cyl_left, cy + box_h / 2.0), egui::Pos2::new(cx + box_w, cy + box_h / 2.0)],
        stroke,
    );
    painter.line_segment(
        [egui::Pos2::new(cx + box_w, cy - box_h / 2.0), egui::Pos2::new(cx + box_w, cy + box_h / 2.0)],
        stroke,
    );

    // Right wire
    painter.line_segment(
        [egui::Pos2::new(cx + box_w, cy), egui::Pos2::new(rect.right() - 4.0, cy)],
        stroke,
    );
}

/// Draw a mass block symbol.
fn draw_mass(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.color_mechanical);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let bw = rect.width() * 0.45;
    let bh = rect.height() * 0.55;

    // Block rectangle
    painter.rect_stroke(
        egui::Rect::from_center_size(egui::Pos2::new(cx, cy), egui::Vec2::new(bw, bh)),
        0.0,
        stroke,
        egui::StrokeKind::Outside,
    );

    // "M" label inside
    let font = egui::FontId::proportional(bh * 0.4);
    painter.text(
        egui::Pos2::new(cx, cy),
        egui::Align2::CENTER_CENTER,
        "M",
        font,
        theme.color_mechanical,
    );

    // Left wire (single flange)
    painter.line_segment(
        [egui::Pos2::new(rect.left() + 4.0, cy), egui::Pos2::new(cx - bw / 2.0, cy)],
        stroke,
    );
}

/// Draw a fixed wall symbol (hatched line).
fn draw_fixed(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.color_mechanical);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let wall_h = rect.height() * 0.6;
    let hatch_count = 4;
    let hatch_len = 6.0;

    // Wire
    painter.line_segment(
        [egui::Pos2::new(rect.left() + 4.0, cy), egui::Pos2::new(cx, cy)],
        stroke,
    );

    // Vertical wall
    painter.line_segment(
        [egui::Pos2::new(cx, cy - wall_h / 2.0), egui::Pos2::new(cx, cy + wall_h / 2.0)],
        egui::Stroke::new(theme.symbol_stroke_width + 1.0, theme.color_mechanical),
    );

    // Hatch marks
    let spacing = wall_h / hatch_count as f32;
    for i in 0..=hatch_count {
        let y = cy - wall_h / 2.0 + i as f32 * spacing;
        painter.line_segment(
            [egui::Pos2::new(cx, y), egui::Pos2::new(cx + hatch_len, y - hatch_len)],
            stroke,
        );
    }
}

/// Draw a gain block (triangle).
fn draw_gain(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.color_signal);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let half_h = rect.height() * 0.4;
    let half_w = rect.width() * 0.35;

    // Triangle: left point, top right, bottom right
    let tri = [
        egui::Pos2::new(cx - half_w, cy - half_h),
        egui::Pos2::new(cx - half_w, cy + half_h),
        egui::Pos2::new(cx + half_w, cy),
    ];
    painter.line_segment([tri[0], tri[1]], stroke);
    painter.line_segment([tri[1], tri[2]], stroke);
    painter.line_segment([tri[2], tri[0]], stroke);

    // Lead wires
    painter.line_segment(
        [egui::Pos2::new(rect.left() + 2.0, cy), egui::Pos2::new(cx - half_w, cy)],
        stroke,
    );
    painter.line_segment(
        [egui::Pos2::new(cx + half_w, cy), egui::Pos2::new(rect.right() - 2.0, cy)],
        stroke,
    );
}

/// Draw an adder symbol (circle with plus).
fn draw_add(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.color_signal);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let radius = rect.height().min(rect.width()) * 0.28;

    painter.circle_stroke(egui::Pos2::new(cx, cy), radius, stroke);
    let s = radius * 0.5;
    painter.line_segment(
        [egui::Pos2::new(cx - s, cy), egui::Pos2::new(cx + s, cy)],
        stroke,
    );
    painter.line_segment(
        [egui::Pos2::new(cx, cy - s), egui::Pos2::new(cx, cy + s)],
        stroke,
    );

    // Lead wires
    painter.line_segment(
        [egui::Pos2::new(rect.left() + 2.0, cy), egui::Pos2::new(cx - radius, cy)],
        stroke,
    );
    painter.line_segment(
        [egui::Pos2::new(cx + radius, cy), egui::Pos2::new(rect.right() - 2.0, cy)],
        stroke,
    );
}

/// Draw an integrator symbol (box with ∫).
fn draw_integrator(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.color_signal);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let bw = rect.width() * 0.45;
    let bh = rect.height() * 0.55;

    painter.rect_stroke(
        egui::Rect::from_center_size(egui::Pos2::new(cx, cy), egui::Vec2::new(bw, bh)),
        0.0,
        stroke,
        egui::StrokeKind::Outside,
    );
    let font = egui::FontId::proportional(bh * 0.5);
    painter.text(
        egui::Pos2::new(cx, cy),
        egui::Align2::CENTER_CENTER,
        "∫",
        font,
        theme.color_signal,
    );

    // Lead wires
    painter.line_segment(
        [egui::Pos2::new(rect.left() + 2.0, cy), egui::Pos2::new(cx - bw / 2.0, cy)],
        stroke,
    );
    painter.line_segment(
        [egui::Pos2::new(cx + bw / 2.0, cy), egui::Pos2::new(rect.right() - 2.0, cy)],
        stroke,
    );
}


/// Dispatch the correct symbol drawing function based on component type.
fn draw_symbol_v2(painter: &egui::Painter, rect: egui::Rect, node: &DiagramNode, theme: &DiagramTheme) {
    // 1. Try to draw SVG icon if present (Authentic Dymola/Modelica look)
    let DiagramNode::Component { icon_asset, .. } = node;
    if let Some(asset_path) = icon_asset {
        let full_path = lunco_assets::msl_dir().join(asset_path);
        if let Ok(svg_data) = std::fs::read(&full_path) {
            super::svg_renderer::draw_svg_to_egui(painter, rect, &svg_data);
            return;
        }
    }

    // 2. Fallback to hardcoded symbols for common components
    let type_name = node.subtitle();
    match type_name {
        "Resistor" => draw_resistor(painter, rect, theme),
        "Capacitor" => draw_capacitor(painter, rect, theme),
        "Inductor" => draw_inductor(painter, rect, theme),
        "Conductor" => draw_resistor(painter, rect, theme), // Conductor reuses zigzag
        "Ground" => draw_ground(painter, rect, theme),
        "ConstantVoltage" => draw_voltage_source(painter, rect, theme),
        "ConstantCurrent" => draw_current_source(painter, rect, theme),
        "VoltageSensor" | "CurrentSensor" => draw_sensor(painter, rect, theme),
        "IdealOpeningSwitch" => draw_switch(painter, rect, theme),
        "Spring" => draw_spring(painter, rect, theme),
        "Damper" => draw_damper(painter, rect, theme),
        "Mass" => draw_mass(painter, rect, theme),
        "Fixed" => draw_fixed(painter, rect, theme),
        "Gain" => draw_gain(painter, rect, theme),
        "Add" => draw_add(painter, rect, theme),
        "Integrator" => draw_integrator(painter, rect, theme),
        "Step" | "Constant" | "Cosh" | "Sinh" | "Tanh" | "Exp" | "Log" | "Log10" | "Sin" | "Cos" | "Tan" | "Asin" | "Acos" | "Atan" | "Atan2" 
            | "Sqrt" | "ABS" | "Sign" => draw_generic_block_v2(painter, rect, node, theme),
        _ => {
             draw_generic_block_v2(painter, rect, node, theme)
        }
    }
}

/// Draw a block-style component symbol (math blocks).
fn draw_generic_block_v2(painter: &egui::Painter, rect: egui::Rect, node: &DiagramNode, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.symbol_color);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let bw = rect.width() * 0.7;
    let bh = rect.height() * 0.7;

    painter.rect_stroke(
        egui::Rect::from_center_size(egui::Pos2::new(cx, cy), egui::Vec2::new(bw, bh)),
        0.0,
        stroke,
        egui::StrokeKind::Outside,
    );

    let type_name = node.subtitle();
    // Use icon_text (e.g. "cosh") or first 8 chars of type name
    let label = match node {
        DiagramNode::Component { icon_text, .. } => icon_text.as_deref().unwrap_or(type_name),
    };
    let label = if label.len() > 10 { &label[..10] } else { label };
    
    let font = egui::FontId::proportional(bh * 0.3);
    painter.text(
        egui::Pos2::new(cx, cy),
        egui::Align2::CENTER_CENTER,
        label,
        font,
        theme.symbol_color,
    );
}


// ---------------------------------------------------------------------------
// Connector-colour helper
// ---------------------------------------------------------------------------

/// Map a connector type string to its domain colour.
fn connector_color(connector_type: &str, theme: &DiagramTheme) -> egui::Color32 {
    match connector_type {
        "Pin" => theme.color_electrical,
        "Flange_a" | "Flange_b" => theme.color_mechanical,
        "RealInput" | "RealOutput" => theme.color_signal,
        _ => theme.color_generic,
    }
}

// ---------------------------------------------------------------------------
// Snarl Viewer — Dymola-style (Phase 1)
// ---------------------------------------------------------------------------

/// The `DiagramViewer` implements the `SnarlViewer` trait to render
/// Modelica components as Dymola-style schematic symbols.
pub struct DiagramViewer<'a> {
    /// Whether to render custom component shapes (true) or generic nodes (false).
    schematic_mode: bool,
    /// Visual theme tokens.
    theme: &'a DiagramTheme,
    /// The rectangle allocated for the snarl canvas in screen space.
    canvas_rect: egui::Rect,
    /// The visible area in graph space (captured from draw_background).
    graph_viewport: Option<egui::Rect>,
    /// Transient storage for the click location within this frame.
    last_click_pos: egui::Pos2,
    /// The currently selected node for inspection.
    selected_node: &'a mut Option<DiagramNodeId>,
    /// Reference to egui context for input checking.
    ctx: egui::Context,
    /// Name of the class the diagram is currently editing (usually the
    /// primary class of the open model — e.g. `"Circuit"` for a file
    /// containing `model Circuit ... end Circuit;`). None when no model
    /// is open; ops are silently skipped in that case.
    editing_class: Option<String>,
    /// AST-level ops emitted inside `SnarlViewer` callbacks. The outer
    /// render loop drains this and applies each op to the bound
    /// document's `DocumentHost` (see Phase α Step 2). Inside viewer
    /// callbacks we only have access to `&mut Snarl<..>`; anything
    /// requiring the world or the DocumentRegistry has to go through
    /// this queue.
    pending_ops: Vec<crate::document::ModelicaOp>,
}

impl<'a> DiagramViewer<'a> {
    /// Push an AST op for the outer loop to apply.
    fn emit_op(&mut self, op: crate::document::ModelicaOp) {
        self.pending_ops.push(op);
    }
}

impl<'a> SnarlViewer<DiagramNode> for DiagramViewer<'a> {

    fn title(&mut self, _node: &DiagramNode) -> String {
        "".to_string()
    }

    fn inputs(&mut self, node: &DiagramNode) -> usize {
        node.port_count()
    }

    fn outputs(&mut self, node: &DiagramNode) -> usize {
        node.port_count()
    }

    // ── Pin rendering — small coloured dots ──

    fn show_input(
        &mut self,
        pin: &InPin,
        _ui: &mut egui::Ui,
        snarl: &mut Snarl<DiagramNode>,
    ) -> impl SnarlPin + 'static {
        let node = &snarl[pin.id.node];
        let ct = node.connector_type_at(pin.id.input).unwrap_or("Pin");
        let color = connector_color(ct, self.theme);

        PinInfo::circle()
            .with_fill(color)
    }

    fn show_output(
        &mut self,
        pin: &OutPin,
        _ui: &mut egui::Ui,
        snarl: &mut Snarl<DiagramNode>,
    ) -> impl SnarlPin + 'static {
        let node = &snarl[pin.id.node];
        let ct = node.connector_type_at(pin.id.output).unwrap_or("Pin");
        let color = connector_color(ct, self.theme);

        PinInfo::circle()
            .with_fill(color)
    }

    // ── Header — instance name + type ──

    // ── Header — disabled to save space (labels moved to body) ──


    fn show_header(
        &mut self,
        _node_id: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        _ui: &mut egui::Ui,
        _snarl: &mut Snarl<DiagramNode>,
    ) {}

    // ── Body — custom component shapes ──
    fn has_body(&mut self, _node: &DiagramNode) -> bool {
        self.schematic_mode
    }


    fn show_body(
        &mut self,
        node_id: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut egui::Ui,
        snarl: &mut Snarl<DiagramNode>,
    ) {
        let node = &snarl[node_id];
        let instance_name = node.title();
        let DiagramNode::Component { id, port_positions, ports: port_names, connector_types: conn_types, .. } = node;
        let port_positions = port_positions.clone();
        let port_names = port_names.clone();
        let conn_types = conn_types.clone();
        
        let body_height = self.theme.body_min_size.y;
        let symbol_size = self.theme.body_min_size.x;

        // Use fixed size for the symbol box
        let (rect, response) = ui.allocate_exact_size(egui::vec2(symbol_size, body_height), egui::Sense::click());
        let painter = ui.painter();

        // 1. Draw manual component border (the "box")
        painter.rect_stroke(rect, 0.0, self.theme.node_stroke, egui::StrokeKind::Middle);
        
        // 2. Fill background slightly to keep it opaque
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(20, 20, 25));

        // 3. Draw schematic symbol inside
        draw_symbol_v2(painter, rect, node, self.theme);

        // 4. Highlight if selected
        if Some(*id) == *self.selected_node {
            painter.rect_stroke(rect.expand(2.0), 0.0, egui::Stroke::new(2.0, egui::Color32::LIGHT_BLUE), egui::StrokeKind::Middle);
        }

        // 5. Draw port dots at their Modelica diagram positions.
        // Modelica coords are -100..100; map onto the body rect.
        // Ports sitting exactly on the boundary (|x|=100 or |y|=100) are drawn
        // as small filled circles with a label so the user knows where to wire.
        for (i, (mx, my)) in port_positions.iter().enumerate() {
            if *mx == 0.0 && *my == 0.0 { continue; } // unknown position, skip
            let px = rect.left() + (mx + 100.0) / 200.0 * rect.width();
            let py = rect.top()  + (100.0 - my) / 200.0 * rect.height();
            let pos = egui::pos2(px, py);
            let ct = conn_types.get(i).map(|s| s.as_str()).unwrap_or("Pin");
            let color = connector_color(ct, self.theme);
            painter.circle_filled(pos, self.theme.port_dot_radius + 1.0, egui::Color32::from_black_alpha(180));
            painter.circle_filled(pos, self.theme.port_dot_radius, color);
            // Port name label — tiny, offset away from boundary
            let label = port_names.get(i).map(|s| s.as_str()).unwrap_or("");
            let offset = egui::vec2(
                if *mx < 0.0 { 8.0 } else if *mx > 0.0 { -8.0 } else { 0.0 },
                if *my > 0.0 { 8.0 } else if *my < 0.0 { -8.0 } else { 0.0 },
            );
            let align = match (*mx as i32, *my as i32) {
                (x, _) if x < 0 => egui::Align2::LEFT_CENTER,
                (x, _) if x > 0 => egui::Align2::RIGHT_CENTER,
                (_, y) if y > 0 => egui::Align2::CENTER_TOP,
                _                => egui::Align2::CENTER_BOTTOM,
            };
            painter.text(
                pos + offset,
                align,
                label,
                egui::FontId::proportional(9.0),
                color,
            );
        }

        // 6. Draw instance label centered below the box
        painter.text(
            egui::pos2(rect.center().x, rect.bottom() + 8.0),
            egui::Align2::CENTER_TOP,
            instance_name,
            egui::FontId::proportional(12.0),
            egui::Color32::from_rgb(220, 220, 230),
        );

        if response.clicked() {
            *self.selected_node = Some(*id);
        }
    }

    fn has_footer(&mut self, _node: &DiagramNode) -> bool {
        false
    }

    // ── Node frame — semi-transparent card ──

    fn node_frame(
        &mut self,
        _default: egui::Frame,
        _node: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        _snarl: &Snarl<DiagramNode>,
    ) -> egui::Frame {
        egui::Frame::NONE
            .inner_margin(0.0)
            .outer_margin(0.0)
    }

    // ── Background — dot grid ──

    fn draw_background(
        &mut self,
        _background: Option<&BackgroundPattern>,
        viewport: &egui::Rect,
        _snarl_style: &SnarlStyle,
        _style: &egui::Style,
        painter: &egui::Painter,
        _snarl: &Snarl<DiagramNode>,
    ) {
        self.graph_viewport = Some(*viewport);

        // Diagnostic: debug draw the canvas rect bounds to see if they align with the panel
        // painter.rect_stroke(self.canvas_rect, 0.0, egui::Stroke::new(1.0, egui::Color32::RED), egui::StrokeKind::Inside);

        let spacing = self.theme.grid_spacing;
        let r = self.theme.grid_dot_radius;
        let color = self.theme.grid_dot_color;

        let x_start = (viewport.min.x / spacing).floor() * spacing;
        let y_start = (viewport.min.y / spacing).floor() * spacing;

        let mut x = x_start;
        while x <= viewport.max.x {
            let mut y = y_start;
            while y <= viewport.max.y {
                painter.circle_filled(egui::Pos2::new(x, y), r, color);
                y += spacing;
            }
            x += spacing;
        }
    }

    // ── Right-click on empty canvas — add components ──

    fn has_graph_menu(&mut self, pos: egui::Pos2, _snarl: &mut Snarl<DiagramNode>) -> bool {
        // [LOCK COORDINATES]
        // IMPORTANT: has_graph_menu receives SCREEN coordinates (interact_pos).
        // show_graph_menu receives transformed GRAPH coordinates (but broken by menu offset).
        // We must map the SCREEN coordinates from has_graph_menu into GRAPH space manually.
        
        let is_clicking = self.ctx.input(|i| {
            i.pointer.secondary_down() || 
            i.pointer.secondary_clicked() || 
            i.pointer.secondary_released()
        });
        
        if is_clicking {
            // Map the screen 'pos' to graph space using our captured transformation
            if let Some(viewport) = self.graph_viewport {
                 let scale_x = viewport.width() / self.canvas_rect.width();
                 let scale_y = viewport.height() / self.canvas_rect.height();
                 
                 let graph_x = (pos.x - self.canvas_rect.min.x) * scale_x + viewport.min.x;
                 let graph_y = (pos.y - self.canvas_rect.min.y) * scale_y + viewport.min.y;
                 
                 self.last_click_pos = egui::Pos2::new(graph_x, graph_y);
            } else {
                 // Fallback if viewport not yet captured
                 self.last_click_pos = pos;
            }
        }
        
        true
    }

    fn show_graph_menu(
        &mut self,
        _pos: egui::Pos2,
        ui: &mut egui::Ui,
        snarl: &mut Snarl<DiagramNode>,
    ) {
        // [PERSISTENT FIX] Use the position we remembered in has_graph_menu.
        // This bypasses the broken _pos argument (12.0 bug) and is stable against mouse movement.
        let pos = self.last_click_pos;

        ui.label(
            egui::RichText::new(format!("➕ Add Component (Graph: {:?})", pos))
                .size(10.0)
                .strong()
                .color(egui::Color32::WHITE),
        );
        ui.separator();

        let categories = msl_categories();
        for cat in &categories {
            let short = cat.split('/').last().unwrap_or(cat);
            ui.menu_button(short, |ui| {
                let components = msl_components_in_category(cat);
                for comp in &components {
                    if ui.button(format!("{} {}", comp.display_name, comp.name)).clicked() {
                        let node = DiagramNode::from_msl(comp);
                        // Pull the auto-generated instance name before
                        // insert_node takes ownership of the node.
                        // If it collides with an existing component
                        // the user will see both in the diagram and a
                        // duplicate-name diagnostic — same behaviour as
                        // OMEdit / Dymola. The user renames manually.
                        let instance_name = match &node {
                            DiagramNode::Component { instance_name, .. } => instance_name.clone(),
                        };
                        snarl.insert_node(pos, node);
                        // Phase α: emit AST op alongside the snarl
                        // mutation. Maps canvas-space `pos` into
                        // Modelica diagram coordinates (+Y up; snarl
                        // uses +Y down so we flip y).
                        match self.editing_class.clone() {
                            Some(class) => self.emit_op(crate::document::ModelicaOp::AddComponent {
                                class,
                                decl: crate::pretty::ComponentDecl {
                                    type_name: comp.msl_path.clone(),
                                    name: instance_name.clone(),
                                    modifications: comp
                                        .parameters
                                        .iter()
                                        .filter(|p| !p.default.is_empty())
                                        .map(|p| (p.name.clone(), p.default.clone()))
                                        .collect(),
                                    placement: Some(crate::pretty::Placement::at(pos.x, -pos.y)),
                                },
                            }),
                            None => warn!(
                                "[Diagram] add `{}` did not emit an AST op — no editing class resolvable",
                                instance_name,
                            ),
                        }
                        ui.close();
                    }
                }
            });
        }
    }

    // ── Right-click on node — delete, etc. ──

    fn has_node_menu(&mut self, _node: &DiagramNode) -> bool {
        true
    }

    fn show_node_menu(
        &mut self,
        node_id: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut egui::Ui,
        snarl: &mut Snarl<DiagramNode>,
    ) {
        let node = &snarl[node_id];
        ui.label(
            egui::RichText::new(format!("{} ({})", node.title(), node.subtitle()))
                .size(11.0)
                .strong(),
        );
        ui.separator();

        if ui.button("🗑 Delete").clicked() {
            // Phase α: emit RemoveComponent to the document before
            // tearing the snarl node. The class context is the
            // primary class of the open model.
            let DiagramNode::Component { instance_name, .. } = &snarl[node_id];
            let instance_name = instance_name.clone();
            match self.editing_class.clone() {
                Some(class) => self.emit_op(crate::document::ModelicaOp::RemoveComponent {
                    class,
                    name: instance_name,
                }),
                None => warn!(
                    "[Diagram] delete `{}` did not emit an AST op — no editing class resolvable",
                    instance_name,
                ),
            }
            snarl.remove_node(node_id);
            ui.close();
        }
    }

    // ── Dropped wire menu — quick-add connected component ──

    fn has_dropped_wire_menu(
        &mut self,
        _src_pins: egui_snarl::ui::AnyPins,
        _snarl: &mut Snarl<DiagramNode>,
    ) -> bool {
        // Capture the interaction origin for dropped wires
        if let Some(click_pos) = self.ctx.input(|i| i.pointer.press_origin()) {
            if let Some(viewport) = self.graph_viewport {
                 let scale_x = viewport.width() / self.canvas_rect.width();
                 let scale_y = viewport.height() / self.canvas_rect.height();
                 let graph_x = (click_pos.x - self.canvas_rect.min.x) * scale_x + viewport.min.x;
                 let graph_y = (click_pos.y - self.canvas_rect.min.y) * scale_y + viewport.min.y;
                 self.last_click_pos = egui::Pos2::new(graph_x, graph_y);
                 println!("[DEBUG] has_dropped_wire_menu locked pos: {:?}", self.last_click_pos);
            }
        }
        true
    }

    fn show_dropped_wire_menu(
        &mut self,
        _pos: egui::Pos2,
        ui: &mut egui::Ui,
        _src_pins: egui_snarl::ui::AnyPins,
        snarl: &mut Snarl<DiagramNode>,
    ) {
        // Use the locked position
        let pos = self.last_click_pos;

        ui.label(
            egui::RichText::new("Connect to new component:")
                .size(11.0)
                .color(egui::Color32::LIGHT_BLUE),
        );
        ui.separator();

        // 1. MSL Categories (The "All Options" request)
        let categories = msl_categories();
        for cat in &categories {
            let short = cat.split('/').last().unwrap_or(cat);
            ui.menu_button(short, |ui| {
                let components = msl_components_in_category(cat);
                for comp in &components {
                    if ui.button(format!("{} {}", comp.display_name, comp.name)).clicked() {
                        let node = DiagramNode::from_msl(comp);
                        snarl.insert_node(pos, node);
                        ui.close();
                    }
                }
            });
        }

        ui.separator();
        ui.label(egui::RichText::new("Common:").size(10.0).color(egui::Color32::GRAY));

        // 2. Quick list for convenience
        let quick = ["Resistor", "Capacitor", "Ground", "ConstantVoltage", "Inductor"];
        for name in &quick {
            if ui.button(*name).clicked() {
                let lib = msl_component_library();
                if let Some(comp) = lib.iter().find(|c| c.name == *name) {
                    let node = DiagramNode::from_msl(comp);
                    snarl.insert_node(pos, node);
                }
                ui.close();
            }
        }
    }

    // ── Tooltip on hover ──

    fn has_on_hover_popup(&mut self, _node: &DiagramNode) -> bool {
        self.schematic_mode
    }

    fn show_on_hover_popup(
        &mut self,
        node_id: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut egui::Ui,
        snarl: &mut Snarl<DiagramNode>,
    ) {
        let node = &snarl[node_id];
        match node {
            DiagramNode::Component { instance_name, type_name, ports, .. } => {
                ui.label(egui::RichText::new(instance_name).strong());
                ui.label(egui::RichText::new(format!("Type: {}", type_name)).size(10.0));
                ui.separator();
                ui.label(egui::RichText::new("Ports:").size(10.0).color(egui::Color32::GRAY));
                for p in ports {
                    ui.label(egui::RichText::new(format!("  • {}", p)).size(10.0));
                }
            }
        }
    }
}

/// Parse Modelica source and build a `VisualDiagram` from component
/// instantiations and `connect()` equations.
///
/// Regex-based fallback scanner for component declarations.
///
/// Returns `(type_path, instance_name)` pairs in source order,
/// including duplicates. Used when rumoca's AST recovery drops
/// components (for example, on a duplicate-name semantic error) so
/// the diagram panel can still render everything the user typed.
///
/// Deliberately simple — this is a best-effort enumerator, not a
/// parser. Skips lines whose first word is a Modelica keyword,
/// skips well-known component prefixes (`flow`, `parameter`, etc.)
/// so they don't shadow the type reference, and matches up to the
/// first `;`, `(`, or newline after the instance name.
fn scan_component_declarations(source: &str) -> Vec<(String, String)> {
    // Matches an optional run of modifier prefixes, then a dotted
    // type path, then the instance name. Uses `\b` (word boundary,
    // zero-width) at the instance-name end so the match doesn't
    // consume any whitespace past the identifier — otherwise a
    // `\s*[\(;\s]` tail will eat the indentation of the *next* line,
    // pulling the iterator past that line's `^` anchor and silently
    // skipping its component. `captures_iter` is non-overlapping, so
    // any whitespace we consume here is unavailable to the next
    // candidate match.
    let re = regex::Regex::new(
        r"(?m)^\s*(?:(?:flow|stream|input|output|parameter|constant|discrete|inner|outer|replaceable|final)\s+)*((?:[A-Za-z_]\w*\.)*[A-Za-z_]\w*)\s+([A-Za-z_]\w*)\b"
    ).expect("scan regex compiles");
    // Keywords that can appear at column 0 inside a class body and
    // therefore look like "type name" starts under a naive regex.
    // When the captured "type" matches one, the match is discarded.
    const KEYWORDS: &[&str] = &[
        "model", "block", "connector", "package", "function", "record", "class", "type",
        "extends", "import", "equation", "algorithm", "initial", "protected", "public",
        "annotation", "connect", "if", "for", "when", "end", "within", "and", "or", "not",
        "true", "false", "else", "elseif", "elsewhen", "while", "loop", "break", "return",
        "then", "external", "encapsulated", "partial", "expandable", "operator", "pure",
        "impure", "redeclare",
    ];
    let mut out = Vec::new();
    for cap in re.captures_iter(source) {
        let ty = cap[1].to_string();
        let inst = cap[2].to_string();
        let first_segment = ty.split('.').next().unwrap_or(&ty);
        if KEYWORDS.contains(&first_segment) {
            continue;
        }
        out.push((ty, inst));
    }
    out
}

/// Build a `VisualDiagram` from scanner-extracted `(type, name)`
/// pairs. Used only when the AST-based path returned nothing — i.e.
/// rumoca failed to produce components for the class. Placement
/// annotations are looked up per-instance via the existing
/// regex-based extractor inside [`import_model_to_diagram`]; here
/// we build a plain grid-layout fallback.
fn build_visual_diagram_from_scan(
    source: &str,
    scanned: &[(String, String)],
    layout: &DiagramAutoLayoutSettings,
) -> VisualDiagram {
    let mut diagram = VisualDiagram::default();
    let msl_lib = msl_component_library();
    let msl_lookup_by_path: HashMap<&str, &MSLComponentDef> = msl_lib
        .iter()
        .map(|c| (c.msl_path.as_str(), c))
        .collect();

    for (idx, (type_path, instance_name)) in scanned.iter().enumerate() {
        // Only render components whose type resolves against the MSL
        // index. Unresolved types stay in the source — the user sees
        // them in the code editor and the parse-error badge — but
        // aren't rendered here because we don't have port info for
        // an unknown type.
        let Some(def) = msl_lookup_by_path.get(type_path.as_str()).cloned() else {
            continue;
        };

        // Placement from annotation (best-effort regex); fall back to grid.
        let safe_name = regex::escape(instance_name);
        let pattern = safe_name
            + r"(?:\s*\([^)]*\))?\s*annotation\s*\(\s*Placement\s*\(\s*transformation\s*\(\s*extent\s*=\s*\{\{\s*([-\d\.]+)\s*,\s*([-\d\.]+)\s*\}\s*,\s*\{\s*([-\d\.]+)\s*,\s*([-\d\.]+)\s*\}\}";
        let annotation_pos = regex::Regex::new(&pattern).ok().and_then(|re| {
            re.captures(source).and_then(|cap| {
                let x1 = cap[1].parse::<f32>().ok()?;
                let y1 = cap[2].parse::<f32>().ok()?;
                let x2 = cap[3].parse::<f32>().ok()?;
                let y2 = cap[4].parse::<f32>().ok()?;
                Some(egui::Pos2::new((x1 + x2) / 2.0, -((y1 + y2) / 2.0)))
            })
        });
        let pos = annotation_pos.unwrap_or_else(|| {
            let cols = layout.cols.max(1);
            let row = idx / cols;
            let col = idx % cols;
            egui::Pos2::new(
                col as f32 * layout.spacing_x,
                row as f32 * layout.spacing_y,
            )
        });

        let node_id = diagram.add_node(def.clone(), pos);
        if let Some(n) = diagram.get_node_mut(node_id) {
            n.instance_name = instance_name.clone();
        }
    }
    diagram
}

/// Default cap for the "don't project absurdly huge models" guard.
/// Catches obvious mistakes (importing a whole MSL subpackage into
/// a diagram viewer) without getting in the way of real engineering
/// models, which typically have a few dozen components and rarely
/// cross a couple hundred.
///
/// Overridable at call time via the `max_nodes` parameter on
/// [`import_model_to_diagram_from_ast`] or the
/// [`DiagramProjectionLimits`] resource the Canvas projection
/// reads. Power users editing a `Magnetic.FundamentalWave` gizmo
/// with 500 components should bump this in Settings, not get a
/// blank canvas.
pub const DEFAULT_MAX_DIAGRAM_NODES: usize = 1000;

/// Returns `None` if the model has no component instantiations
/// (e.g., equation-based models like Battery.mo, SpringMass.mo).
pub fn import_model_to_diagram(source: &str) -> Option<VisualDiagram> {
    // Delegate to the AST-taking variant after parsing once. Keeps
    // existing callers working while letting hot paths (Canvas
    // projection) reuse an already-parsed AST from `ModelicaDocument`.
    let syntax = rumoca_phase_parse::parse_to_syntax(source, "model.mo");
    let ast: rumoca_session::parsing::ast::StoredDefinition = syntax.best_effort().clone();
    import_model_to_diagram_from_ast(
        std::sync::Arc::new(ast),
        source,
        DEFAULT_MAX_DIAGRAM_NODES,
        None,
        &DiagramAutoLayoutSettings::default(),
    )
}

/// Same as [`import_model_to_diagram`] but reuses an already-
/// parsed AST. Saves two full rumoca passes (one in the component-
/// builder, one in the imports-resolution path). Used by the
/// canvas's async projection task where
/// `ModelicaDocument::ast()` already holds the parsed tree.
///
/// `max_nodes` is a guard against accidentally projecting a huge
/// package (e.g. `Modelica.Units`) into a diagram — returns `None`
/// if the parsed graph exceeds the cap. See
/// [`DEFAULT_MAX_DIAGRAM_NODES`] for the conventional value; the
/// canvas projection reads it from `DiagramProjectionLimits` so
/// users editing deeply composed models can raise it in Settings.
pub fn import_model_to_diagram_from_ast(
    ast: std::sync::Arc<rumoca_session::parsing::ast::StoredDefinition>,
    source: &str,
    max_nodes: usize,
    target_class: Option<&str>,
    layout: &DiagramAutoLayoutSettings,
) -> Option<VisualDiagram> {
    use crate::diagram::ModelicaComponentBuilder;
    // `Arc::clone` here is a pointer bump, NOT a tree clone.
    // MSL package ASTs are megabytes; a naïve clone would push the
    // process into swap on drill-in into anything under
    // `Modelica/Blocks/package.mo` etc.
    //
    // `target_class` scopes the builder to a specific class inside
    // the AST — critical for drill-in tabs backed by multi-class
    // package files. Without it, the builder would walk every
    // sibling class (dozens in `Blocks/package.mo`) and render a
    // Frankenstein diagram. With it, we get only the drilled-in
    // class's components and connect equations.
    let mut builder = ModelicaComponentBuilder::from_ast(std::sync::Arc::clone(&ast));
    if let Some(target) = target_class {
        builder = builder.target_class(target);
    }
    let graph = builder.build();

    // If the AST-based graph has no components, fall back to a
    // source-text scan before concluding the model is equation-only.
    //
    // Why: rumoca's error recovery drops *all* components of a class
    // when it hits a semantic error like a duplicate name (per
    // MLS, duplicates are a namespace violation). An OMEdit /
    // Dymola-style editor must still render what the user wrote so
    // they can fix the error — returning `None` here leaves them
    // staring at a blank canvas with no clue *why*.
    //
    // The scanner is regex-based and deliberately simple; it catches
    // the common `<Qualified.Type> <InstanceName>[(mods)] [;/anno];`
    // shape but doesn't pretend to be a full Modelica parser. When the
    // AST is healthy (the 99% case), this fallback never runs.
    //
    // **Critical**: the scanner reads the WHOLE source, so it has
    // no notion of class scoping. We only run it when no
    // `target_class` was specified — drill-in tabs into a specific
    // class inside a package file MUST NOT trigger this fallback,
    // or they end up displaying every sibling class's components
    // jumbled together. Honor the scope the caller asked for.
    if graph.node_count() == 0 {
        if target_class.is_some() {
            return None;
        }
        let scanned = scan_component_declarations(source);
        if !scanned.is_empty() {
            return Some(build_visual_diagram_from_scan(source, &scanned, layout));
        }
        return None;
    }

    // Safety: prevent projecting absurdly huge packages (e.g.
    // `Modelica.Units` with thousands of type declarations) as a
    // diagram. The cap is caller-supplied so power users editing
    // rich composed models can raise it via Settings; default is
    // `DEFAULT_MAX_DIAGRAM_NODES`.
    if graph.node_count() > max_nodes {
        warn!(
            "[Diagram] Model exceeds node cap ({} > {}). Skipping diagram generation. \
             Raise `Settings → Diagram → Max nodes` to project anyway.",
            graph.node_count(),
            max_nodes,
        );
        return None;
    }

    // Convert ComponentGraph → VisualDiagram
    let mut diagram = VisualDiagram::default();

    // MSL lookup table — keyed by the fully-qualified Modelica path
    // (e.g. `"Modelica.Blocks.Continuous.Integrator"`).
    //
    // Type resolution follows MLS §5.3: a component's `type_name` is
    // matched against its containing class's import table and any
    // enclosing scopes. Our pretty-printer always emits fully-qualified
    // paths, so that route resolves directly. For short-name
    // references we build a per-class import map from the parsed AST
    // below.
    //
    // Short-name-tail heuristics (e.g. `Integrator` → first MSL entry
    // whose path ends in `.Integrator`) are *not* applied — MSL has
    // multiple classes sharing short names (for example,
    // `Modelica.Blocks.Continuous.Integrator` vs.
    // `Modelica.Blocks.Continuous.Integrator` nested variants), and
    // matching by suffix would silently pick the wrong one. If a
    // reference doesn't resolve via scope or path, we surface it as
    // unresolved (skipped) rather than guess.
    let msl_lib = msl_component_library();
    let msl_lookup_by_path: HashMap<&str, &MSLComponentDef> = msl_lib.iter()
        .map(|c| (c.msl_path.as_str(), c))
        .collect();

    // Build the active class's import map so we can resolve
    // short-name type references the way OpenModelica's frontend
    // does. We re-parse the source here (cheap — the cache is warm
    // from the component-graph builder above) so we can walk
    // `ClassDef.imports` for each top-level class.
    //
    // Format:  short_name → fully_qualified_path
    // Covers `Qualified` (C → A.B.C), `Renamed` (D = A.B.C → D → A.B.C),
    // and `Selective` (import A.B.{C,D} → C → A.B.C, D → A.B.D).
    // `Unqualified` (A.B.*) is not expanded here because it would
    // require a second pass against the whole MSL index; that's a
    // separate follow-up.
    let mut imports_by_short: HashMap<String, String> = HashMap::new();
    // Reuse the `ast` argument instead of re-parsing the source.
    // The fake `if let Ok(ast) = _` wrapper used to shadow; now we
    // just take a borrow of the already-parsed tree.
    {
        let ast = &ast;
        for (_class_name, class_def) in ast.classes.iter() {
            for imp in &class_def.imports {
                use rumoca_session::parsing::ast::Import;
                match imp {
                    Import::Qualified { path, .. } => {
                        let full = path.to_string();
                        if let Some(last) = full.rsplit('.').next() {
                            imports_by_short.insert(last.to_string(), full.clone());
                        }
                    }
                    Import::Renamed { alias, path, .. } => {
                        imports_by_short.insert(alias.text.to_string(), path.to_string());
                    }
                    Import::Selective { path, names, .. } => {
                        let base = path.to_string();
                        for name in names {
                            imports_by_short.insert(
                                name.text.to_string(),
                                format!("{}.{}", base, name.text),
                            );
                        }
                    }
                    Import::Unqualified { .. } => {
                        // `import Pkg.*;` — expansion needs the full
                        // package contents. Deferred.
                    }
                }
            }
        }
    }

    // Local same-file class lookup, keyed by short name.
    //
    // Modelica scope rules (MLS §5.3) make sibling classes inside a
    // package directly visible to one another without an `import`. The
    // MSL palette only knows about MSL paths, so user classes defined
    // alongside the model (e.g. `Engine`/`Tank` inside an
    // `AnnotatedRocketStage` package) would otherwise resolve as
    // unknown and disappear from the diagram. We synthesise a
    // [`MSLComponentDef`] for each top-level class and one nesting
    // level deeper, carrying the extracted `Icon` annotation so the
    // canvas can render the user's own graphics.
    //
    // Ports are intentionally empty here — connector extraction for
    // user classes is a follow-up; the icon-rendering slice doesn't
    // need them.
    let mut local_classes_by_short: HashMap<String, MSLComponentDef> = HashMap::new();
    for (top_name, top_class) in ast.classes.iter() {
        register_local_class(&mut local_classes_by_short, top_name.as_str(), top_class);
        for (nested_name, nested_class) in top_class.classes.iter() {
            register_local_class(
                &mut local_classes_by_short,
                nested_name.as_str(),
                nested_class,
            );
        }
    }

    // Index every component in the projection scope by short name so
    // the layout loop can walk rumoca's typed `annotation: Vec<Expression>`
    // for each instance instead of pattern-matching source text.
    // Scope is the target_class when set (drill-in tab), else every
    // class in the file — same scope the source-text regex used to
    // operate on.
    let comp_by_short: HashMap<&str, &rumoca_session::parsing::ast::Component> = {
        let mut map: HashMap<&str, &rumoca_session::parsing::ast::Component> =
            HashMap::new();
        if let Some(target) = target_class {
            // Scope to the named class — search top-level and
            // nested. First exact-name match wins (Modelica scope
            // rules guarantee uniqueness inside a class).
            'find: for (top_name, top) in ast.classes.iter() {
                if top_name.as_str() == target {
                    for (cname, comp) in top.components.iter() {
                        map.insert(cname.as_str(), comp);
                    }
                    break 'find;
                }
                if let Some(nested) = top.classes.get(target) {
                    for (cname, comp) in nested.components.iter() {
                        map.insert(cname.as_str(), comp);
                    }
                    break 'find;
                }
            }
        } else {
            for (_n, top) in ast.classes.iter() {
                for (cname, comp) in top.components.iter() {
                    map.insert(cname.as_str(), comp);
                }
                for (_nn, nested) in top.classes.iter() {
                    for (cname, comp) in nested.components.iter() {
                        map.insert(cname.as_str(), comp);
                    }
                }
            }
        }
        map
    };

    // Place nodes in a sparse grid as fallback for components without
    // a `Placement` annotation. Wide enough that orthogonal wires
    // get room to bend without colliding with neighbours; alternating
    // half-row offsets stagger neighbouring rows so ports on the
    // shared horizontal band don't end up wired through the body of
    // the row above. Matches the breathing room Dymola/OMEdit's
    // default layout uses for un-annotated example models.
    // Stable per-component slot: each graph node's position in the
    // list defines its fallback offset, regardless of whether siblings
    // have a `Placement` annotation. Without this, annotating one
    // component shifts every un-annotated sibling.
    for (node_idx, node) in graph.nodes.iter().enumerate() {
        if node.qualified_name.is_empty() {
            continue;
        }

        // Extract short name from qualified_name (e.g., "RC_Circuit.R1" → "R1")
        let short_name = node.qualified_name.split('.').last().unwrap_or(&node.qualified_name);

        // Scope-aware type lookup:
        //   1. `type_name` looks like a fully-qualified path → match directly.
        //   2. `type_name` is a single segment → consult the class's
        //      import table; if present, substitute the resolved full
        //      path and look that up.
        //   3. Otherwise: unresolved. Skip (same as an OM compile error
        //      on an unknown type, but non-fatal here).
        let type_name = node.meta.get("type_name").map(|s| s.as_str()).unwrap_or("");
        let resolved_path: Option<&str> = if type_name.contains('.') {
            Some(type_name)
        } else if let Some(full) = imports_by_short.get(type_name) {
            Some(full.as_str())
        } else {
            None
        };
        let component_def: Option<MSLComponentDef> = resolved_path
            .and_then(|p| msl_lookup_by_path.get(p).map(|d| (*d).clone()))
            .or_else(|| local_classes_by_short.get(type_name).cloned());

        if let Some(def) = component_def {
            let mut pos = None;
            // Build the full icon-local → canvas affine in one place.
            // Falls back to a default transform centred on the grid
            // position below when no Placement is authored.
            let mut icon_transform: Option<crate::icon_transform::IconTransform> = None;

            // Read placement from rumoca's typed annotation tree
            // instead of pattern-matching source text. Robust against
            // whitespace, comments, multi-line layouts, and handles
            // origin/rotation correctly. Falls through to the grid
            // fallback below when no Placement is authored.
            if let Some(comp) = comp_by_short.get(short_name) {
                if let Some(placement) =
                    crate::annotations::extract_placement(&comp.annotation)
                {
                    let extent = placement.transformation.extent;
                    let cx = ((extent.p1.x + extent.p2.x) * 0.5) as f32;
                    let cy = ((extent.p1.y + extent.p2.y) * 0.5) as f32;
                    let ox = placement.transformation.origin.x as f32;
                    let oy = placement.transformation.origin.y as f32;
                    let mirror_x = extent.p2.x < extent.p1.x;
                    let mirror_y = extent.p2.y < extent.p1.y;
                    let size = (
                        (extent.p2.x - extent.p1.x).abs() as f32,
                        (extent.p2.y - extent.p1.y).abs() as f32,
                    );
                    let rotation_deg = placement.transformation.rotation as f32;
                    let xform = crate::icon_transform::IconTransform::from_placement(
                        (cx, cy),
                        size,
                        mirror_x,
                        mirror_y,
                        rotation_deg,
                        (ox, oy),
                    );
                    // Cached centre matches where the icon-local
                    // origin lands in canvas world coords.
                    let (px, py) = xform.apply(0.0, 0.0);
                    pos = Some(egui::Pos2::new(px, py));
                    icon_transform = Some(xform);
                }
            }

            // Fallback when no `Placement` annotation: deterministic
            // grid keyed by the node's AST index. Index-stable — an
            // annotated sibling never shifts un-annotated ones —
            // while staying visually usable without the user having
            // to click Auto-Arrange first.
            let pos = pos.unwrap_or_else(|| {
                let cols = layout.cols.max(1);
                let row = node_idx / cols;
                let col = node_idx % cols;
                let row_shift = if row % 2 == 1 {
                    layout.spacing_x * layout.row_stagger
                } else {
                    0.0
                };
                egui::Pos2::new(
                    col as f32 * layout.spacing_x + row_shift,
                    row as f32 * layout.spacing_y,
                )
            });

            let node_id = diagram.add_node(def.clone(), pos);

            if let Some(diagram_node) = diagram.get_node_mut(node_id) {
                diagram_node.instance_name = short_name.to_string();
                if let Some(xf) = icon_transform {
                    diagram_node.icon_transform = xf;
                }
            }
        }
    }

    // Add edges from graph connections
    for edge in &graph.edges {
        let src_node = &graph.nodes[edge.source.0 as usize];
        let tgt_node = &graph.nodes[edge.target.0 as usize];

        let src_short = src_node.qualified_name.split('.').last().unwrap_or("");
        let tgt_short = tgt_node.qualified_name.split('.').last().unwrap_or("");

        // Find matching diagram nodes
        let src_diagram_id = diagram.nodes.iter()
            .find(|n| n.instance_name == src_short)
            .map(|n| n.id);
        let tgt_diagram_id = diagram.nodes.iter()
            .find(|n| n.instance_name == tgt_short)
            .map(|n| n.id);

        if let (Some(src_id), Some(tgt_id)) = (src_diagram_id, tgt_diagram_id) {
            // Port names from graph node ports
            let src_port = src_node.ports.get(edge.source_port).map(|p| p.name.clone()).unwrap_or_default();
            let tgt_port = tgt_node.ports.get(edge.target_port).map(|p| p.name.clone()).unwrap_or_default();
            diagram.add_edge(src_id, src_port, tgt_id, tgt_port);
        }
    }

    if diagram.nodes.is_empty() {
        None
    } else {
        Some(diagram)
    }
}

/// Add a synthesised palette entry for a class found in the open
/// document. Used for short-name resolution of sibling classes that
/// the MSL palette doesn't know about. Skips classes that don't carry
/// any of the data we'd render — i.e. no decoded `Icon` annotation.
fn register_local_class(
    out: &mut HashMap<String, MSLComponentDef>,
    short_name: &str,
    class_def: &rumoca_session::parsing::ast::ClassDef,
) {
    use crate::annotations::extract_icon;
    if out.contains_key(short_name) {
        return;
    }
    let icon = extract_icon(&class_def.annotation);
    if icon.is_none() {
        return;
    }
    out.insert(
        short_name.to_string(),
        MSLComponentDef {
            name: short_name.to_string(),
            msl_path: short_name.to_string(),
            category: "Local".to_string(),
            display_name: short_name.to_string(),
            description: None,
            icon_text: None,
            icon_asset: None,
            ports: Vec::new(),
            parameters: Vec::new(),
            icon_graphics: icon,
        },
    );
}

// ---------------------------------------------------------------------------
// Diagram ↔ Snarl Sync
// ---------------------------------------------------------------------------

fn build_snarl(diagram: &VisualDiagram) -> Snarl<DiagramNode> {
    let mut snarl = Snarl::default();
    let mut id_map: HashMap<DiagramNodeId, NodeId> = HashMap::new();

    for node in &diagram.nodes {
        let ports: Vec<String> = node.component_def.ports.iter().map(|p| p.name.clone()).collect();
        let connector_types: Vec<String> = node.component_def.ports.iter().map(|p| p.connector_type.clone()).collect();
        let port_positions: Vec<(f32, f32)> = node.component_def.ports.iter().map(|p| (p.x, p.y)).collect();
        let snarl_node = DiagramNode::Component {
            id: node.id,
            instance_name: node.instance_name.clone(),
            type_name: node.component_def.name.clone(),
            description: node.component_def.description.clone(),
            icon_text: node.component_def.icon_text.clone(),
            icon_asset: node.component_def.icon_asset.clone(),
            ports,
            connector_types,
            port_positions,
        };
        let pos = egui::Pos2::new(node.position.x, node.position.y);
        let sid = snarl.insert_node(pos, snarl_node);
        id_map.insert(node.id, sid);
    }

    for edge in &diagram.edges {
        if let (Some(&src_sid), Some(&tgt_sid)) = (id_map.get(&edge.source_node), id_map.get(&edge.target_node)) {
            let src_node = diagram.get_node(edge.source_node);
            let tgt_node = diagram.get_node(edge.target_node);
            if let (Some(src), Some(tgt)) = (src_node, tgt_node) {
                let src_idx = src.component_def.ports.iter().position(|p| p.name == edge.source_port).unwrap_or(0);
                let tgt_idx = tgt.component_def.ports.iter().position(|p| p.name == edge.target_port).unwrap_or(0);
                snarl.connect(
                    OutPinId { node: src_sid, output: src_idx },
                    InPinId { node: tgt_sid, input: tgt_idx },
                );
            }
        }
    }

    snarl
}

/// Synchronise snarl state back into the canonical `VisualDiagram`.
///
/// Handles node deletions (via `remove_node` in snarl context menus),
/// position updates from drags, and edge reconciliation from wire edits.
/// Also reconciles nodes added via graph/dropped-wire menus that only
/// exist in the snarl but not yet in the diagram.
fn sync_connections(snarl: &Snarl<DiagramNode>, diagram: &mut VisualDiagram) {
    // 1. Reconcile: add any snarl nodes that are missing from the diagram
    //    (created via right-click → Add Component on the canvas).
    for (_sid, _pos, snarl_node) in snarl.nodes_pos_ids() {
        let DiagramNode::Component { id, type_name, .. } = snarl_node;
        let exists = diagram.nodes.iter().any(|n| n.id == *id);
        if !exists {
                // Look up the MSL def for this type
                let msl_lib = msl_component_library();
                if let Some(def) = msl_lib.iter().find(|c| c.name == *type_name) {
                    println!("Sync: adding missing node {:?} to diagram", id);
                    diagram.add_node_with_id(*id, def.clone(), egui::Pos2::new(_pos.x, _pos.y));
                }
            }
        }

    // 2. Remove diagram nodes that no longer exist in snarl (deleted via menu).
    diagram.nodes.retain(|n| {
        snarl.nodes().any(|snarl_node| {
            let DiagramNode::Component { id, .. } = snarl_node;
            *id == n.id
        })
    });

    // 3. Update positions from snarl drag results.
    let grid_spacing = 15.0;
    for (_sid, pos, snarl_node) in snarl.nodes_pos_ids() {
        let DiagramNode::Component { id, .. } = snarl_node;
        if let Some(diagram_node) = diagram.get_node_mut(*id) {
            let snapped_x = (pos.x / grid_spacing).round() * grid_spacing;
            let snapped_y = (pos.y / grid_spacing).round() * grid_spacing;
            diagram_node.position = egui::Pos2::new(snapped_x, snapped_y);
        }
    }

    // 4. Rebuild edges from snarl wires.
    diagram.edges.clear();
    for (out_pin, in_pin) in snarl.wires() {
        let src_sid = out_pin.node;
        let tgt_sid = in_pin.node;
        let src_pidx = out_pin.output;
        let tgt_pidx = in_pin.input;

        let DiagramNode::Component { id: src_id, ports: src_ports, .. } = &snarl[src_sid];
        let DiagramNode::Component { id: tgt_id, ports: tgt_ports, .. } = &snarl[tgt_sid];
        let src_port = src_ports.get(src_pidx).cloned().unwrap_or_default();
        let tgt_port = tgt_ports.get(tgt_pidx).cloned().unwrap_or_default();
        diagram.add_edge(*src_id, src_port, *tgt_id, tgt_port);
    }
}

/// Read all wires from snarl as a set of unordered `(component, port)`
/// pairs. Canonicalised so `(A.p, B.n)` and `(B.n, A.p)` hash to the
/// same entry — Modelica `connect(...)` is symmetric, so swapping
/// endpoints shouldn't count as a different connection.
///
/// Used by the wire-diff logic in the panel render loop to detect
/// newly-drawn / newly-removed wires between frames.
fn read_wire_set(
    snarl: &Snarl<DiagramNode>,
) -> std::collections::HashSet<((String, String), (String, String))> {
    let mut out = std::collections::HashSet::new();
    for (out_pin, in_pin) in snarl.wires() {
        let DiagramNode::Component { instance_name: src_name, ports: src_ports, .. } =
            &snarl[out_pin.node];
        let DiagramNode::Component { instance_name: tgt_name, ports: tgt_ports, .. } =
            &snarl[in_pin.node];
        let src_port = match src_ports.get(out_pin.output) {
            Some(p) => p.clone(),
            None => continue,
        };
        let tgt_port = match tgt_ports.get(in_pin.input) {
            Some(p) => p.clone(),
            None => continue,
        };
        let a = (src_name.clone(), src_port);
        let b = (tgt_name.clone(), tgt_port);
        // Canonicalise: smaller endpoint first.
        if a <= b {
            out.insert((a, b));
        } else {
            out.insert((b, a));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Diagram Panel
// ---------------------------------------------------------------------------

/// Diagram canvas panel — Dymola-style visual editor for Modelica models.
pub struct DiagramPanel;

impl Panel for DiagramPanel {
    fn id(&self) -> PanelId { PanelId("modelica_diagram_preview") }
    fn title(&self) -> String { "🔗 Diagram".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::Center }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        if world.get_resource::<DiagramState>().is_none() {
            world.insert_resource(DiagramState::default());
        }
        if world.get_resource::<DiagramTheme>().is_none() {
            world.insert_resource(DiagramTheme::default());
        }
        if world.get_resource::<DiagramAutoLayoutSettings>().is_none() {
            world.insert_resource(DiagramAutoLayoutSettings::default());
        }

        // ── Phase α: track the active document ──
        //
        // The diagram is always editing *some* document. Whenever the
        // open_model changes we rebind so the change-stream cursor
        // resets and the next sync consumes the document's AST from
        // scratch. Legacy import path still runs below during the
        // transition; future work folds the import into a single
        // document-driven rebuild triggered by `changes_since(0)`
        // returning `None` (the fresh-bind sentinel).
        {
            let open_doc = world
                .get_resource::<lunco_workbench::WorkspaceResource>()
                .and_then(|ws| ws.active_document);
            if let Some(mut ds) = world.get_resource_mut::<DiagramState>() {
                match open_doc {
                    Some(id) => ds.bind_document(id),
                    None => ds.unbind_document(),
                }
            }
        }

        // ── Phase α Step 3: document-driven snarl sync ──
        //
        // Whenever the bound document's generation has advanced past
        // `last_seen_gen` — either because another panel (code
        // editor) committed a ReplaceSource / EditText, because a
        // fresh document just bound, or because the retention window
        // dropped entries we'd have needed — we rebuild snarl from
        // the document's current source.
        //
        // Our own ops emitted in the previous frame also bump the
        // generation; to avoid a rebuild ping-pong we explicitly
        // advance `last_seen_gen` after applying ops at the bottom of
        // this render function (see `advance_cursor_after_apply`).
        //
        // The rebuild is synchronous — re-parsing a small Modelica
        // file takes well under a frame budget. The old
        // `AsyncComputeTaskPool`-driven background parse is gone
        // (and with it the "Analyzing model structure..." spinner
        // that flashed on every edit).
        {
            let sync_needed = {
                let gen = world
                    .get_resource::<crate::ui::ModelicaDocumentRegistry>()
                    .and_then(|r| {
                        world
                            .get_resource::<DiagramState>()
                            .and_then(|ds| ds.document)
                            .and_then(|doc| r.host(doc))
                            .map(|h| h.generation())
                    });
                let last = world
                    .get_resource::<DiagramState>()
                    .map(|ds| ds.last_seen_gen)
                    .unwrap_or(0);
                gen.map(|g| g != last).unwrap_or(false)
            };

            if sync_needed {
                let source = world
                    .get_resource::<DiagramState>()
                    .and_then(|ds| ds.document)
                    .and_then(|doc| {
                        world
                            .get_resource::<crate::ui::ModelicaDocumentRegistry>()
                            .and_then(|r| r.host(doc))
                            .map(|h| h.document().source().to_string())
                    });
                let new_gen = world
                    .get_resource::<DiagramState>()
                    .and_then(|ds| ds.document)
                    .and_then(|doc| {
                        world
                            .get_resource::<crate::ui::ModelicaDocumentRegistry>()
                            .and_then(|r| r.host(doc))
                            .map(|h| h.generation())
                    })
                    .unwrap_or(0);
                if let (Some(src), Some(mut ds)) =
                    (source, world.get_resource_mut::<DiagramState>())
                {
                    // Synchronous rebuild. `import_model_to_diagram`
                    // parses + walks the AST + emits a VisualDiagram;
                    // we then rebuild snarl from it via the existing
                    // `build_snarl` helper (invoked inside
                    // `rebuild_snarl`).
                    ds.diagram = import_model_to_diagram(&src).unwrap_or_default();
                    ds.rebuild_snarl();
                    // Diff caches are now stale — they referenced
                    // snarl node state from before the rebuild. Clear
                    // so the next frame establishes a fresh baseline
                    // instead of firing spurious Add/Remove/SetPlacement
                    // ops.
                    ds.last_wires = read_wire_set(&ds.snarl);
                    ds.last_positions = None;
                    ds.last_seen_gen = new_gen;
                    ds.compile_status = None;
                }
                // Legacy: the package browser still flips
                // `diagram_dirty` on model switch; clear it so the
                // flag doesn't linger after our sync has already
                // handled the change.
                if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
                    state.diagram_dirty = false;
                }
            }
        }

        // ── Empty-diagram policy ──
        //
        // A class with no components can mean two very different
        // things. We route by writability, not by emptiness:
        //
        // - **Read-only** (MSL library entries, bundled examples that
        //   happen to be equation-based like RocketEngine / Battery /
        //   BouncyBall): the user can't edit them anyway, so render
        //   the class as a **single Icon-style block** — a preview of
        //   how this model would look if dropped into a parent
        //   diagram. That's [`render_equation_only_empty_state`].
        //
        // - **Writable** (a freshly-created file, a user model with
        //   no components yet): render the **empty editable canvas**
        //   so the user can immediately drag components from the
        //   palette / right-click to add. Showing a single Icon block
        //   here is a dead-end — it collapses the diagram into a
        //   non-interactive preview of an empty class, which is
        //   exactly backwards for authoring.
        //
        // The right mental model is: the diagram is always "the
        // inside of this class." Only when the class is a black box
        // we can't edit does it make sense to show it from the
        // outside.
        let is_diagram_empty = world
            .get_resource::<DiagramState>()
            .map(|ds| ds.diagram.nodes.is_empty())
            .unwrap_or(true);
        let is_read_only = world
            .get_resource::<WorkbenchState>()
            .and_then(|s| s.open_model.as_ref())
            .map(|m| m.read_only)
            .unwrap_or(false);
        if is_diagram_empty && is_read_only {
            render_equation_only_empty_state(ui, world);
            return;
        }

        // Read-only models render the same snarl canvas but wrapped
        // in a disabled UI scope: the user can pan/zoom/select to
        // inspect structure, but drag, drops, right-click menus, and
        // parameter edits are all blocked. This is stricter than
        // "mutations silently fail" — inputs don't register at all.
        //
        // The visual grayed-out state from `add_enabled_ui(false)` is
        // itself the signal: read-only mode is visible on the canvas,
        // not only in a toolbar badge.

        // Body only. Identity (model name, read-only state), compile
        // button and view-mode switching all live on the ModelView panel's
        // unified toolbar. The Schematic/NodeGraph toggle and diagram-
        // specific stats will migrate up as a contextual sub-toolbar in a
        // follow-up; for now schematic mode stays at its persisted value
        // and users can flip it via code / script / future menu entry.


        // ── Canvas (egui-snarl) ──
        ui.set_min_height(600.0);
        // Read theme + schematic mode
        let theme = world.get_resource::<DiagramTheme>()
            .cloned()
            .unwrap_or_default();
        let schematic_mode = world.get_resource::<DiagramState>()
            .map(|ds| ds.schematic_mode)
            .unwrap_or(true);

        let mut snarl_style = SnarlStyle::default();
        snarl_style.pin_size = Some(theme.port_dot_radius * 2.0);
        snarl_style.collapsible = Some(false);
        snarl_style.header_drag_space = Some(egui::Vec2::ZERO);
        snarl_style.header_frame = Some(egui::Frame::NONE);
        snarl_style.pin_placement = Some(egui_snarl::ui::PinPlacement::Edge);
        snarl_style.node_layout = Some(egui_snarl::ui::NodeLayout {
            kind: egui_snarl::ui::NodeLayoutKind::Coil,
            min_pin_row_height: 0.0,
            ..Default::default()
        });
        snarl_style.wire_width = Some(2.0);
        snarl_style.wire_style = Some(egui_snarl::ui::WireStyle::AxisAligned {
            corner_radius: 0.0, // Sharp corners for authentic Dymola look
        });

        // ── Canvas (egui-snarl) ──
        let available_size = ui.available_size().max(egui::vec2(100.0, 500.0));
        let (rect, _) = ui.allocate_exact_size(available_size, egui::Sense::hover());

        // Pull the editing-class name so AST-level ops know where to
        // target. Prefer the live document AST (authoritative — reflects
        // renames and content changes immediately), fall back to the
        // open_model's memoized display name, and finally to the first
        // top-level class in the parsed AST.
        //
        // Without a class name the diagram panel still mutates snarl
        // but silently drops AST-op emission — the whole Phase α loop
        // collapses. So we bend over backwards to produce *some* name.
        let mut pending_ops: Vec<crate::document::ModelicaOp> = Vec::new();
        let bound_doc = world
            .get_resource::<DiagramState>()
            .and_then(|ds| ds.document);
        let editing_class: Option<String> = bound_doc
            .and_then(|id| {
                world
                    .get_resource::<crate::ui::ModelicaDocumentRegistry>()
                    .and_then(|r| r.host(id))
                    .and_then(|h| {
                        h.document()
                            .ast()
                            .ast()
                            .and_then(|s| s.classes.keys().next().cloned())
                    })
            })
            .or_else(|| {
                world
                    .get_resource::<WorkbenchState>()
                    .and_then(|s| s.open_model.as_ref())
                    .and_then(|m| m.detected_name.clone())
            });

        if let Some(mut ds) = world.get_resource_mut::<DiagramState>() {
            let mut selected_node = ds.selected_node;
            let mut viewer = DiagramViewer {
                schematic_mode,
                theme: &theme,
                canvas_rect: rect,
                graph_viewport: None,
                last_click_pos: ds.last_click_pos,
                selected_node: &mut selected_node,
                ctx: ui.ctx().clone(),
                editing_class: editing_class.clone(),
                pending_ops: Vec::new(),
            };

            // Render snarl inside the allocated rect using a stable child UI
            // The rect.min must match the panel's start point for correct coordinate mapping.
            // We only wrap in a `disable()` scope when the model is
            // read-only; on writable docs we call snarl directly so
            // no extra UI layer can intercept / throttle input.
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |child_ui| {
                child_ui.set_clip_rect(rect);
                if is_read_only {
                    child_ui.add_enabled_ui(false, |child_ui| {
                        ds.snarl.show(&mut viewer, &snarl_style, "diagram", child_ui);
                    });
                } else {
                    ds.snarl.show(&mut viewer, &snarl_style, "diagram", child_ui);
                }
            });

            // Save back the click position and selection if they were updated
            ds.last_click_pos = viewer.last_click_pos;
            pending_ops = viewer.pending_ops;
            ds.selected_node = selected_node;

            // Phase α: position diff — emit `SetPlacement` for any
            // component whose snarl position changed between frames.
            // Snarl's graph space is +Y down; Modelica's diagram space
            // is +Y up, so we flip y on the way out.
            //
            // On the first frame after bind (`last_positions = None`)
            // we only record the baseline and emit nothing, so the
            // initial render after an import doesn't replay SetPlacement
            // for every component (which would thrash annotations
            // that round-trip from source in a slightly different
            // coordinate convention).
            {
                let mut now_positions: std::collections::HashMap<String, (f32, f32)> =
                    std::collections::HashMap::new();
                for (_id, pos, node) in ds.snarl.nodes_pos_ids() {
                    let DiagramNode::Component { instance_name, .. } = node;
                    now_positions.insert(instance_name.clone(), (pos.x, -pos.y));
                }
                match &ds.last_positions {
                    Some(prev) => {
                        if let Some(class) = editing_class.as_ref() {
                            for (name, (x, y)) in &now_positions {
                                let moved = match prev.get(name) {
                                    Some((px, py)) => (*px - x).abs() > 0.5 || (*py - y).abs() > 0.5,
                                    None => false, // newly added — AddComponent already carried its placement
                                };
                                if moved {
                                    pending_ops.push(crate::document::ModelicaOp::SetPlacement {
                                        class: class.clone(),
                                        name: name.clone(),
                                        placement: crate::pretty::Placement::at(*x, *y),
                                    });
                                }
                            }
                        }
                    }
                    None => {}
                }
                ds.last_positions = Some(now_positions);
            }

            // Phase α: wire diff — snarl is the authoring truth, the
            // document catches up.
            //
            // Read the current wire set from snarl, canonicalise each
            // wire as an *unordered* pair so the order of endpoints
            // doesn't flip between `AddConnection` calls, and compare
            // to `last_wires` from the previous frame:
            //
            //   - wires in `now` but not `last`  →  user drew a wire
            //   - wires in `last` but not `now`  →  user disconnected
            //
            // We emit `AddConnection` / `RemoveConnection` per diff
            // entry; the editing-class is resolved once and re-used.
            let now_wires = read_wire_set(&ds.snarl);
            if now_wires != ds.last_wires {
                if let Some(class) = editing_class.as_ref() {
                    for pair in now_wires.difference(&ds.last_wires) {
                        pending_ops.push(crate::document::ModelicaOp::AddConnection {
                            class: class.clone(),
                            eq: crate::pretty::ConnectEquation {
                                from: crate::pretty::PortRef::new(&pair.0.0, &pair.0.1),
                                to: crate::pretty::PortRef::new(&pair.1.0, &pair.1.1),
                                line: None,
                            },
                        });
                    }
                    for pair in ds.last_wires.difference(&now_wires) {
                        pending_ops.push(crate::document::ModelicaOp::RemoveConnection {
                            class: class.clone(),
                            from: crate::pretty::PortRef::new(&pair.0.0, &pair.0.1),
                            to: crate::pretty::PortRef::new(&pair.1.0, &pair.1.1),
                        });
                    }
                }
                ds.last_wires = now_wires;
            }

            // Sync
            let DiagramState { snarl, diagram, .. } = &mut *ds;
            sync_connections(snarl, diagram);
        }

        // Drain AST ops emitted by the viewer and apply them to the
        // bound document. Read-only models silently skip (the enabled
        // guard above prevents emission in the first place, but the
        // check here catches any stray ops from non-UI paths).
        //
        // After any successful ops land, mirror the document's new
        // source back to `WorkbenchState.open_model.source` so other
        // panels that read that cached string (code editor, library
        // breadcrumb, etc.) immediately see the change. This is a
        // transitional bridge — once the code editor reads the
        // document directly, the write-back goes away.
        if !pending_ops.is_empty() && !is_read_only {
            if let Some(doc_id) = bound_doc {
                let mut any_applied = false;
                if let Some(mut registry) =
                    world.get_resource_mut::<crate::ui::ModelicaDocumentRegistry>()
                {
                    if let Some(host) = registry.host_mut(doc_id) {
                        for op in pending_ops {
                            match host.apply(op) {
                                Ok(_) => any_applied = true,
                                Err(e) => warn!("[Diagram] AST op failed: {}", e),
                            }
                        }
                    }
                }
                if any_applied {
                    // Mirror the fresh source back to the cached
                    // `open_model.source` so the code editor and any
                    // other panel reading that field sees the update.
                    let fresh = world
                        .get_resource::<crate::ui::ModelicaDocumentRegistry>()
                        .and_then(|r| r.host(doc_id))
                        .map(|h| (h.document().source().to_string(), h.generation()));
                    if let Some((src, new_gen)) = fresh {
                        if let Some(mut ws) = world.get_resource_mut::<WorkbenchState>() {
                            if let Some(open) = ws.open_model.as_mut() {
                                // Recompute line starts alongside the
                                // source update — the code editor
                                // reads these for layout/navigation.
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
                        // Advance the sync cursor past our own ops so
                        // the next frame's Step-3 sync doesn't
                        // rebuild snarl in response to changes we
                        // just produced. The viewer already applied
                        // them to snarl directly.
                        if let Some(mut ds) = world.get_resource_mut::<DiagramState>() {
                            ds.last_seen_gen = new_gen;
                            // `last_wires` / `last_positions` were
                            // updated from snarl state earlier in
                            // this same frame, so they already
                            // reflect the post-edit snarl. No reset
                            // needed here.
                        }
                    }
                }
            }
        }

        // The Compile button lives on the ModelViewPanel unified toolbar
        // now; in Diagram mode it calls `do_compile` below directly.
    }
}

/// Execute the diagram-to-compile workflow for the *bound* document:
/// read its current source → write temp file → spawn or reuse a
/// `ModelicaModel` entity linked to that document → send
/// [`ModelicaCommand::Compile`] to the worker.
///
/// Public so [`crate::ui::panels::model_view::ModelViewPanel`] can
/// dispatch it from the unified toolbar when the user is in Diagram
/// mode.
///
/// Phase α retires the old regenerate-from-VisualDiagram path that
/// synthesised a fresh `VisualModel{n}.mo` on every compile. Every
/// compile now targets the document the diagram is editing (stored on
/// `DiagramState.document`) and uses its authoritative source —
/// including whatever the code editor has typed and whatever the
/// diagram's AST ops have spliced in.
pub fn do_compile(world: &mut World) {
    // 1. Resolve the bound document + its current source + class name.
    let Some(doc_id) = world
        .get_resource::<DiagramState>()
        .and_then(|s| s.document)
    else {
        if let Some(mut s) = world.get_resource_mut::<DiagramState>() {
            s.compile_status = Some("No document bound to diagram".into());
            s.compile_ok = false;
        }
        return;
    };

    let (source, model_name) = {
        let Some(registry) = world.get_resource::<crate::ui::ModelicaDocumentRegistry>() else {
            return;
        };
        let Some(host) = registry.host(doc_id) else {
            if let Some(mut s) = world.get_resource_mut::<DiagramState>() {
                s.compile_status = Some("Bound document missing from registry".into());
                s.compile_ok = false;
            }
            return;
        };
        let doc = host.document();
        let source = doc.source().to_string();
        let name = doc
            .ast()
            .ast()
            .and_then(|s| s.classes.keys().next().cloned())
            .unwrap_or_else(|| "Model".into());
        (source, name)
    };

    // 2. Write the source to a temp file for the compiler worker.
    let temp_dir = std::env::temp_dir().join("luncosim");
    let _ = std::fs::create_dir_all(&temp_dir);
    let temp_path = temp_dir.join(format!("{}.mo", model_name));
    if let Err(e) = std::fs::write(&temp_path, &source) {
        if let Some(mut s) = world.get_resource_mut::<DiagramState>() {
            s.compile_status = Some(format!("Write error: {}", e));
            s.compile_ok = false;
        }
        return;
    }

    // 3. Bump the session counter (used as a fence for stale results).
    let session_id = {
        let mut s = world.resource_mut::<DiagramState>();
        s.model_counter = s.model_counter.saturating_add(1);
        s.model_counter as u64
    };

    // 4. Spawn the entity and link it to the existing document.
    let entity = world
        .spawn((
            Name::new(model_name.clone()),
            ModelicaModel {
                model_path: temp_path,
                model_name: model_name.clone(),
                current_time: 0.0,
                last_step_time: 0.0,
                session_id,
                paused: false,
                parameters: HashMap::new(),
                inputs: HashMap::new(),
                variables: HashMap::new(),
                descriptions: HashMap::new(),
                document: doc_id,
                is_stepping: true,
            },
        ))
        .id();

    world
        .resource_mut::<crate::ui::ModelicaDocumentRegistry>()
        .link(entity, doc_id);

    // 5. Mark the document as compiling so UI chips / disabled states
    //    reflect in-flight work.
    world
        .resource_mut::<crate::ui::CompileStates>()
        .set(doc_id, crate::ui::CompileState::Compiling);

    // 6. Dispatch to the worker.
    if let Some(channels) = world.get_resource::<ModelicaChannels>() {
        let _ = channels.tx.send(ModelicaCommand::Compile {
            entity,
            session_id,
            model_name,
            source,
        });
    }
    if let Some(mut s) = world.get_resource_mut::<DiagramState>() {
        s.compile_status = Some("Compiling…".into());
    }
}

/// Cached per-document signature used by the equation-only empty
/// state. Populated lazily the first time we render this view for a
/// (doc, generation) pair; invalidated when the generation bumps.
///
/// Shipped because the naive version re-ran `extract_parameters` +
/// `extract_inputs_with_defaults` + `extract_input_names` every
/// frame, regex-scanning the full source at 60 fps. On anything but
/// a trivial model that crushed rendering — caching drops it to one
/// scan per edit.
#[derive(Resource, Default)]
pub struct ModelSignatureCache {
    by_doc: std::collections::HashMap<lunco_doc::DocumentId, CachedSignature>,
}

struct CachedSignature {
    generation: u64,
    sig: ModelSignature,
}

/// Structural summary of a Modelica model — what its parameters,
/// inputs, and observable variables are. Rendered as an icon-style
/// block when the model has no visual components to draw.
pub struct ModelSignature {
    pub params: Vec<(String, f64)>,
    pub inputs: Vec<(String, Option<f64>)>,
    pub observables: Vec<String>,
}

impl ModelSignatureCache {
    /// Return the cached signature for `doc`, recomputing if the
    /// document's generation changed since the last query.
    pub fn get_or_compute(
        &mut self,
        doc: lunco_doc::DocumentId,
        generation: u64,
        source: &str,
    ) -> &ModelSignature {
        let needs_rebuild = self
            .by_doc
            .get(&doc)
            .map(|c| c.generation != generation)
            .unwrap_or(true);
        if needs_rebuild {
            let sig = compute_signature(source);
            self.by_doc.insert(doc, CachedSignature { generation, sig });
        }
        &self.by_doc.get(&doc).unwrap().sig
    }
}

fn compute_signature(source: &str) -> ModelSignature {
    let params_map = crate::ast_extract::extract_parameters(source);
    let mut params: Vec<(String, f64)> =
        params_map.into_iter().collect();
    params.sort_by(|a, b| a.0.cmp(&b.0));

    let inputs_with_defaults = crate::ast_extract::extract_inputs_with_defaults(source);
    let runtime_inputs = crate::ast_extract::extract_input_names(source);
    let mut inputs: Vec<(String, Option<f64>)> = inputs_with_defaults
        .iter()
        .map(|(k, v)| (k.clone(), Some(*v)))
        .chain(
            runtime_inputs
                .iter()
                .filter(|n| !inputs_with_defaults.contains_key(*n))
                .map(|n| (n.clone(), None)),
        )
        .collect();
    inputs.sort_by(|a, b| a.0.cmp(&b.0));

    // Observables: `Real X`, `Integer X`, `Boolean X` declarations
    // that aren't parameters or inputs. Quick line-based scan — not
    // AST-accurate, but good enough for a tab-header preview.
    let mut observables: Vec<String> = Vec::new();
    let param_keys: std::collections::HashSet<_> =
        params.iter().map(|(k, _)| k.clone()).collect();
    let input_keys: std::collections::HashSet<_> =
        inputs.iter().map(|(k, _)| k.clone()).collect();
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("parameter") || trimmed.starts_with("input") {
            continue;
        }
        for kw in ["Real ", "Integer ", "Boolean "] {
            if let Some(rest) = trimmed.strip_prefix(kw) {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty()
                    && !param_keys.contains(&name)
                    && !input_keys.contains(&name)
                    && !observables.contains(&name)
                {
                    observables.push(name);
                }
                break;
            }
        }
    }

    ModelSignature {
        params,
        inputs,
        observables,
    }
}

/// Shown when the active model has no visual components — i.e.
/// equation-based (RocketEngine, Battery, BouncyBall, SpringMass).
/// Renders the model as an **icon-style block** (Modelica `Icon`
/// convention): rounded rect with title, input port dots on the left,
/// observable port dots on the right. Matches how the model would
/// look if dropped into another diagram as a component.
fn render_equation_only_empty_state(ui: &mut egui::Ui, world: &mut World) {
    // Read active doc id from the Workspace session (source of truth)
    // before borrowing WorkbenchState for the display cache.
    let active_doc = world
        .get_resource::<lunco_workbench::WorkspaceResource>()
        .and_then(|ws| ws.active_document);
    let (model_name, doc_id, generation, source) = {
        let state = world.resource::<WorkbenchState>();
        let Some(open) = state.open_model.as_ref() else {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.heading("🔗 No model open");
                ui.label(
                    egui::RichText::new(
                        "Pick a model from the sidebar or the Welcome tab.",
                    )
                    .size(12.0)
                    .color(egui::Color32::GRAY),
                );
            });
            return;
        };
        let Some(doc) = active_doc else {
            // Open but no DocumentId yet (rare transient) — don't
            // bother with signature; show minimal state.
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.heading(format!("🔗 {}", open.display_name));
                ui.label(
                    egui::RichText::new("Preparing…")
                        .size(11.0)
                        .color(egui::Color32::GRAY),
                );
            });
            return;
        };
        let generation = world
            .get_resource::<crate::ui::state::ModelicaDocumentRegistry>()
            .and_then(|r| r.host(doc))
            .map(|h| h.generation())
            .unwrap_or(0);
        (
            open.display_name.clone(),
            doc,
            generation,
            open.source.to_string(),
        )
    };

    // Ensure the cache resource exists (panels run with &mut World
    // and can introduce resources lazily).
    if world.get_resource::<ModelSignatureCache>().is_none() {
        world.insert_resource(ModelSignatureCache::default());
    }

    // Clone-out the signature so we can release the cache borrow
    // before drawing (drawing doesn't need the cache, and we'd
    // rather not hold a ResMut across the full painter pass).
    let (params, inputs, observables) = {
        let mut cache = world.resource_mut::<ModelSignatureCache>();
        let sig = cache.get_or_compute(doc_id, generation, &source);
        (
            sig.params.clone(),
            sig.inputs.clone(),
            sig.observables.clone(),
        )
    };

    // Pull per-variable description strings from the compiled model, if
    // any entity is linked to this document. Populated by the worker
    // on compile-type results (see `handle_modelica_responses`). Empty
    // when the user hasn't compiled yet — tooltips just no-op.
    let descriptions: std::collections::HashMap<String, String> = world
        .resource::<crate::ui::state::ModelicaDocumentRegistry>()
        .entities_linked_to(doc_id)
        .into_iter()
        .find_map(|e| {
            world
                .get::<crate::ModelicaModel>(e)
                .map(|m| m.descriptions.clone())
        })
        .unwrap_or_default();

    draw_model_icon_block(ui, &model_name, &params, &inputs, &observables, &descriptions);
}

/// Paint a Modelica "Icon"-style block centered in the current
/// panel. Rounded rect, title bar at top, input ports on the left,
/// observable ports on the right, parameters listed inside.
///
/// No interaction — the block is read-only visualization. Future:
/// draggable onto another diagram, rename via double-click, etc.
fn draw_model_icon_block(
    ui: &mut egui::Ui,
    name: &str,
    params: &[(String, f64)],
    inputs: &[(String, Option<f64>)],
    observables: &[String],
    descriptions: &std::collections::HashMap<String, String>,
) {
    let avail = ui.available_rect_before_wrap();
    let port_rows = inputs.len().max(observables.len()) as f32;
    let param_rows = params.len() as f32;

    // Block size scales to content, centered in the view.
    let block_w = 420.0_f32.min(avail.width() - 40.0).max(260.0);
    let title_h = 36.0;
    let port_h = 20.0;
    let param_padding = if param_rows > 0.0 { 10.0 } else { 0.0 };
    let content_h = (port_h * port_rows.max(1.0))
        + (16.0 * param_rows)
        + param_padding;
    let block_h = (title_h + content_h + 24.0).max(180.0);

    let center = avail.center();
    let block_rect = egui::Rect::from_center_size(
        egui::pos2(center.x, (avail.top() + 60.0 + block_h / 2.0).min(center.y)),
        egui::vec2(block_w, block_h),
    );

    let painter = ui.painter();

    // Body + title bar.
    let bg = egui::Color32::from_rgb(38, 42, 52);
    let border = egui::Color32::from_rgb(120, 140, 180);
    let title_bg = egui::Color32::from_rgb(60, 70, 92);
    painter.rect_filled(block_rect, 10.0, bg);
    painter.rect_stroke(
        block_rect,
        10.0,
        egui::Stroke::new(1.5, border),
        egui::StrokeKind::Outside,
    );
    let title_rect =
        egui::Rect::from_min_size(block_rect.min, egui::vec2(block_rect.width(), title_h));
    painter.rect_filled(
        title_rect,
        egui::CornerRadius {
            nw: 10,
            ne: 10,
            sw: 0,
            se: 0,
        },
        title_bg,
    );
    painter.text(
        title_rect.center(),
        egui::Align2::CENTER_CENTER,
        name,
        egui::FontId::proportional(14.5),
        egui::Color32::WHITE,
    );

    // Port colors (match Palette category conventions roughly).
    let input_color = egui::Color32::from_rgb(230, 190, 100); // amber
    let output_color = egui::Color32::from_rgb(120, 200, 120); // green
    let label_color = egui::Color32::from_rgb(220, 220, 220);
    let muted = egui::Color32::from_rgb(160, 160, 170);

    // Inputs on the left edge.
    //
    // Each port gets both a painter-drawn visual (dot + label) and an
    // interactive rect covering the dot plus the label's full extent,
    // so hovering shows the variable's Modelica description string as
    // a tooltip. Painter draws don't produce egui Responses on their
    // own; allocating an invisible rect is the standard workaround.
    let content_top = title_rect.bottom() + 14.0;
    let hover_rects: Vec<(egui::Rect, String)> = {
        let mut rects = Vec::new();
        for (i, (name, _default)) in inputs.iter().enumerate() {
            let y = content_top + i as f32 * port_h + port_h * 0.5;
            let pos = egui::pos2(block_rect.left(), y);
            painter.circle_filled(pos, 4.5, input_color);
            painter.text(
                pos + egui::vec2(8.0, 0.0),
                egui::Align2::LEFT_CENTER,
                name,
                egui::FontId::monospace(11.0),
                label_color,
            );
            // Rect covers ~140px of label + the dot.
            let r = egui::Rect::from_min_max(
                egui::pos2(pos.x - 6.0, y - 8.0),
                egui::pos2(pos.x + 150.0, y + 8.0),
            );
            rects.push((r, name.clone()));
        }
        for (i, name) in observables.iter().enumerate() {
            let y = content_top + i as f32 * port_h + port_h * 0.5;
            let pos = egui::pos2(block_rect.right(), y);
            painter.circle_filled(pos, 4.5, output_color);
            painter.text(
                pos + egui::vec2(-8.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                name,
                egui::FontId::monospace(11.0),
                label_color,
            );
            let r = egui::Rect::from_min_max(
                egui::pos2(pos.x - 150.0, y - 8.0),
                egui::pos2(pos.x + 6.0, y + 8.0),
            );
            rects.push((r, name.clone()));
        }
        rects
    };

    // Parameters listed in the middle below ports, centered.
    let mut param_hover_rects: Vec<(egui::Rect, String)> = Vec::new();
    if !params.is_empty() {
        let params_top = content_top + port_h * port_rows.max(1.0) + 8.0;
        painter.text(
            egui::pos2(block_rect.center().x, params_top),
            egui::Align2::CENTER_TOP,
            format!("parameters ({})", params.len()),
            egui::FontId::proportional(10.0),
            muted,
        );
        for (i, (k, v)) in params.iter().enumerate() {
            let y = params_top + 14.0 + i as f32 * 14.0;
            painter.text(
                egui::pos2(block_rect.center().x, y),
                egui::Align2::CENTER_TOP,
                format!("{} = {}", k, v),
                egui::FontId::monospace(10.5),
                label_color,
            );
            let r = egui::Rect::from_min_max(
                egui::pos2(block_rect.center().x - 150.0, y),
                egui::pos2(block_rect.center().x + 150.0, y + 14.0),
            );
            param_hover_rects.push((r, k.clone()));
        }
    }

    // Claim the block area so egui knows we drew something (prevents
    // the panel from collapsing to zero height).
    ui.allocate_rect(block_rect, egui::Sense::hover());

    // Stamp each port/param hover rect on top. We use `interact_at`
    // (not `allocate_rect`) so the rects overlap the block without
    // disturbing layout. When the description is absent the hover
    // still fires; `.on_hover_text` is a no-op with empty text.
    for (rect, var) in hover_rects.into_iter().chain(param_hover_rects.into_iter()) {
        let id = ui.id().with(("icon_block_hover", &var));
        let resp = ui.interact(rect, id, egui::Sense::hover());
        if let Some(desc) = descriptions.get(&var) {
            resp.on_hover_text(desc);
        }
    }

    // Below the block, a short explainer — the user can still read
    // this line if they're new to why the canvas is "empty".
    let below_y = block_rect.bottom() + 16.0;
    ui.painter().text(
        egui::pos2(center.x, below_y),
        egui::Align2::CENTER_TOP,
        "Equation-only model — switch to 📝 Text to read or 🚀 Compile (F5) to simulate.",
        egui::FontId::proportional(11.0),
        muted,
    );
}

