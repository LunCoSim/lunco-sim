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
// Diagram State
// ---------------------------------------------------------------------------

/// Resource holding the visual diagram being built on the canvas.
#[derive(Resource)]
pub struct DiagramState {
    /// The visual model being built.
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
}

impl DiagramState {
    /// Add a component to both the diagram data and the snarl UI.
    pub fn add_component(&mut self, def: MSLComponentDef, pos: egui::Pos2) {
        let node_id = self.diagram.add_node(def.clone(), pos);
        let ports: Vec<String> = def.ports.iter().map(|p| p.name.clone()).collect();
        let connector_types: Vec<String> = def.ports.iter().map(|p| p.connector_type.clone()).collect();

        let snarl_node = DiagramNode::Component {
            id: node_id,
            instance_name: self.diagram.get_node(node_id).unwrap().instance_name.clone(),
            type_name: def.name.clone(),
            description: def.description.clone(),
            icon_text: def.icon_text.clone(),
            icon_asset: def.icon_asset.clone(),
            ports,
            connector_types,
        };

        self.snarl.insert_node(pos, snarl_node);
    }

    /// Rebuild the snarl from the current diagram.
    pub fn rebuild_snarl(&mut self) {
        self.snarl = build_snarl(&self.diagram);
    }
}

impl Default for DiagramState {
    fn default() -> Self {
        Self {
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
    },
}

impl DiagramNode {
    fn from_msl(comp: &MSLComponentDef) -> Self {
        let ports: Vec<String> = comp.ports.iter().map(|p| p.name.clone()).collect();
        let connector_types: Vec<String> = comp.ports.iter().map(|p| p.connector_type.clone()).collect();
        DiagramNode::Component {
            id: DiagramNodeId::new(),
            instance_name: format!("New{}", comp.name),
            type_name: comp.name.clone(),
            description: comp.description.clone(),
            icon_text: comp.icon_text.clone(),
            icon_asset: comp.icon_asset.clone(),
            ports,
            connector_types,
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
        let DiagramNode::Component { id, .. } = node;
        
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

        // 5. Draw instance label centered below the box
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
                        // Place node at click.
                        println!("Inserting node at graph pos: {:?}", pos);
                        snarl.insert_node(pos, node);
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
/// Returns `None` if the model has no component instantiations
/// (e.g., equation-based models like Battery.mo, SpringMass.mo).
fn import_model_to_diagram(source: &str) -> Option<VisualDiagram> {
    use crate::diagram::ModelicaComponentBuilder;

    // Try to build a component graph from the source
    let builder = ModelicaComponentBuilder::from_source(source)?;
    let graph = builder.build();

    // If no components found, this is an equation-based model
    if graph.node_count() == 0 {
        return None;
    }

    // Safety: prevent importing massive packages (like Units) as diagrams
    if graph.node_count() > 100 {
        warn!("[Diagram] Model too complex ({} nodes). Skipping diagram generation.", graph.node_count());
        return None;
    }

    // Convert ComponentGraph → VisualDiagram
    let mut diagram = VisualDiagram::default();

    // Build a lookup from component type name → MSLComponentDef
    let msl_lib = msl_component_library();
    let msl_lookup: HashMap<&str, &MSLComponentDef> = msl_lib.iter()
        .map(|c| (c.name.as_str(), c))
        .collect();

    // Place nodes in a grid layout (fallback for unannotated components)
    let node_spacing_x = 200.0;
    let node_spacing_y = 150.0;
    let cols = 3;
    let mut placement_idx = 0;

    for node in graph.nodes.iter() {
        if node.qualified_name.is_empty() {
            continue;
        }

        // Extract short name from qualified_name (e.g., "RC_Circuit.R1" → "R1")
        let short_name = node.qualified_name.split('.').last().unwrap_or(&node.qualified_name);

        // Try to find matching MSL component definition
        let type_name = node.meta.get("type_name").map(|s| s.as_str()).unwrap_or("");
        let component_def = msl_lookup.get(type_name)
            .or_else(|| msl_lookup.get(short_name))
            .cloned();

        if let Some(def) = component_def {
            let mut pos = None;
            
            // Try to find annotation coordinate for this instance
            let safe_name = regex::escape(short_name);
            let pattern = safe_name + r"(?:\s*\([^)]*\))?\s+annotation\s*\(\s*Placement\s*\(\s*transformation\s*\(\s*extent\s*=\s*\{\{\s*([-\d\.]+)\s*,\s*([-\d\.]+)\s*\}\s*,\s*\{\s*([-\d\.]+)\s*,\s*([-\d\.]+)\s*\}\}\s*\)\s*\)\s*\)";
            if let Ok(re) = regex::Regex::new(&pattern) {
                if let Some(cap) = re.captures(source) {
                    if let (Ok(x1), Ok(y1), Ok(x2), Ok(y2)) = (
                        cap[1].parse::<f32>(),
                        cap[2].parse::<f32>(),
                        cap[3].parse::<f32>(),
                        cap[4].parse::<f32>(),
                    ) {
                        let x = (x1 + x2) / 2.0;
                        let y = -((y1 + y2) / 2.0); // Modelica is +UP, Snarl is +DOWN
                        pos = Some(egui::Pos2::new(x, y));
                    }
                }
            }

            // Fallback to grid pos if no annotation
            let pos = pos.unwrap_or_else(|| {
                let row = placement_idx / cols;
                let col = placement_idx % cols;
                placement_idx += 1;
                egui::Pos2::new(col as f32 * node_spacing_x, row as f32 * node_spacing_y)
            });

            let node_id = diagram.add_node(def.clone(), pos);

            // Override the auto-generated instance name with the one from the source
            if let Some(diagram_node) = diagram.get_node_mut(node_id) {
                diagram_node.instance_name = short_name.to_string();
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

// ---------------------------------------------------------------------------
// Diagram ↔ Snarl Sync
// ---------------------------------------------------------------------------

fn build_snarl(diagram: &VisualDiagram) -> Snarl<DiagramNode> {
    let mut snarl = Snarl::default();
    let mut id_map: HashMap<DiagramNodeId, NodeId> = HashMap::new();

    for node in &diagram.nodes {
        let ports: Vec<String> = node.component_def.ports.iter().map(|p| p.name.clone()).collect();
        let connector_types: Vec<String> = node.component_def.ports.iter().map(|p| p.connector_type.clone()).collect();
        let snarl_node = DiagramNode::Component {
            id: node.id,
            instance_name: node.instance_name.clone(),
            type_name: node.component_def.name.clone(),
            description: node.component_def.description.clone(),
            icon_text: node.component_def.icon_text.clone(),
            icon_asset: node.component_def.icon_asset.clone(),
            ports,
            connector_types,
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

        // ── Check if open_model changed → trigger import task ──
        //
        // When the Package Browser opens a new model, it sets
        // `diagram_dirty = true`. We spawn the parse task *and* clear the
        // current visual state so the user sees either the reparsed
        // diagram or an empty canvas — never a stale carry-over from a
        // previously-open file or a user-built diagram that no longer
        // belongs to the active source.
        {
            let dirty = world.get_resource::<WorkbenchState>()
                .map(|s| s.diagram_dirty)
                .unwrap_or(false);
            if dirty {
                let source = world.get_resource::<WorkbenchState>()
                    .and_then(|s| s.open_model.as_ref().map(|m| m.source.clone()));
                if let Some(mut ds) = world.get_resource_mut::<DiagramState>() {
                    ds.diagram = VisualDiagram::default();
                    ds.snarl = egui_snarl::Snarl::default();
                    ds.compile_status = None;
                }
                if let Some(source) = source {
                    let pool = bevy::tasks::AsyncComputeTaskPool::get();
                    let task = pool.spawn(async move {
                        import_model_to_diagram(&source)
                    });
                    if let Some(mut ds) = world.get_resource_mut::<DiagramState>() {
                        ds.parse_task = Some(task);
                    }
                }
                if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
                    state.diagram_dirty = false;
                }
            }
        }

        // ── Poll import task ──
        //
        // Treat a `None` result as "no diagram representable from this
        // source" — keep the cleared canvas so the view honestly reflects
        // the active model rather than silently falling back to old state.
        let mut is_parsing = false;
        if let Some(mut ds) = world.get_resource_mut::<DiagramState>() {
            if let Some(mut task) = ds.parse_task.take() {
                if let Some(diagram_opt) = futures_lite::future::block_on(futures_lite::future::poll_once(&mut task)) {
                    ds.diagram = diagram_opt.unwrap_or_default();
                    ds.rebuild_snarl();
                } else {
                    ds.parse_task = Some(task);
                    is_parsing = true;
                }
            }
        }

        if is_parsing {
            ui.vertical_centered(|ui| {
                ui.add_space(100.0);
                ui.spinner();
                ui.heading("Analyzing model structure...");
                ui.label(egui::RichText::new("Parsing Modelica AST for diagram visualization").size(10.0).color(egui::Color32::GRAY));
            });
            return;
        }

        // ── Equation-only empty state ──
        //
        // Models like RocketEngine / Battery are pure equations, with
        // no component instantiations or `connect` statements. There
        // is nothing to draw — `import_model_to_diagram` returned
        // `None`, so `ds.diagram.nodes` is empty. Rather than render a
        // silent blank canvas, show the user *why* nothing is there
        // and preview the model's shape (parameters, inputs,
        // observables) so the Diagram view still communicates
        // something.
        //
        // Users with components that *should* show up see the canvas
        // as normal; this branch only triggers when there's genuinely
        // nothing visual.
        let is_diagram_empty = world
            .get_resource::<DiagramState>()
            .map(|ds| ds.diagram.nodes.is_empty())
            .unwrap_or(true);
        if is_diagram_empty {
            render_equation_only_empty_state(ui, world);
            return;
        }

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
            };

            // Render snarl inside the allocated rect using a stable child UI
            // The rect.min must match the panel's start point for correct coordinate mapping
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |child_ui| {
                child_ui.set_clip_rect(rect);
                ds.snarl.show(&mut viewer, &snarl_style, "diagram", child_ui);
            });

            // Save back the click position and selection if they were updated
            ds.last_click_pos = viewer.last_click_pos;
            ds.selected_node = selected_node;

            // Sync
            let DiagramState { snarl, diagram, .. } = &mut *ds;
            sync_connections(snarl, diagram);
        }

        // The Compile button lives on the ModelViewPanel unified toolbar
        // now; in Diagram mode it calls `do_compile` below directly.
    }
}

/// Execute the diagram-to-compile workflow: generate Modelica source from
/// the current [`DiagramState`] → write temp file → spawn or update a
/// `ModelicaModel` entity → send [`ModelicaCommand::Compile`] to the
/// worker. Public so [`crate::ui::panels::model_view::ModelViewPanel`]
/// can dispatch it from the unified toolbar when the user is in
/// Diagram mode.
pub fn do_compile(world: &mut World) {
    // Extract data first
    let (model_counter, source, temp_path) = {
        let Some(s) = world.get_resource::<DiagramState>() else { return };
        let mc = s.model_counter + 1;
        let model_name = format!("VisualModel{}", mc);
        let source = generate_modelica_source(&s.diagram, &model_name);
        let temp_dir = std::env::temp_dir().join("luncosim");
        let _ = std::fs::create_dir_all(&temp_dir);
        let temp_path = temp_dir.join(format!("{}.mo", model_name));
        (mc, source, temp_path)
    };

    // Write file
    if let Err(e) = std::fs::write(&temp_path, &source) {
        if let Some(mut s) = world.get_resource_mut::<DiagramState>() {
            s.compile_status = Some(format!("Write error: {}", e));
            s.compile_ok = false;
        }
        return;
    }

    // Allocate a Document up-front so the new entity is spawned with a
    // valid `document` id pointing at the source we're about to compile.
    let doc_id = world
        .resource_mut::<crate::ui::ModelicaDocumentRegistry>()
        .allocate(source.clone());

    // Spawn entity
    let session_id = model_counter as u64;
    let model_name = format!("VisualModel{}", model_counter);
    let entity = world.spawn((
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
    )).id();

    world
        .resource_mut::<crate::ui::ModelicaDocumentRegistry>()
        .link(entity, doc_id);

    // Mark the document as compiling — UI (status chips, disabled button)
    // reads this to reflect in-flight state.
    world
        .resource_mut::<crate::ui::CompileStates>()
        .set(doc_id, crate::ui::CompileState::Compiling);

    // Send command
    if let Some(channels) = world.get_resource::<ModelicaChannels>() {
        let _ = channels.tx.send(ModelicaCommand::Compile {
            entity,
            session_id,
            model_name,
            source,
        });
    }
    if let Some(mut s) = world.get_resource_mut::<DiagramState>() {
        s.model_counter = model_counter;
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
        let doc = open.doc;
        let Some(doc) = doc else {
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

