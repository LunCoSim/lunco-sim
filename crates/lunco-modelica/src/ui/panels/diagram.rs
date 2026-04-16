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
            grid_dot_radius: 1.0,
            grid_dot_color: egui::Color32::from_gray(55),

            node_bg: egui::Color32::from_rgba_premultiplied(20, 20, 25, 200),
            node_stroke: egui::Stroke::new(1.0, egui::Color32::from_rgb(50, 50, 60)),
            node_rounding: 2,

            symbol_stroke_width: 2.0,
            symbol_color: egui::Color32::from_rgb(200, 200, 210),
            body_min_size: egui::Vec2::new(100.0, 100.0),

            port_dot_radius: 5.0,
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
            type_name: def.name,
            description: def.description,
            icon_text: def.icon_text,
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
    let w = rect.width() * 0.8;
    let h = rect.height() * 0.35;
    let half_w = w / 2.0;
    let segments = 5;
    let seg_w = w / segments as f32;

    // Lead-in wire (left)
    let start_x = cx - half_w - seg_w;
    painter.line_segment(
        [egui::Pos2::new(start_x, cy), egui::Pos2::new(cx - half_w, cy)],
        stroke,
    );

    // Zigzag
    let mut points = Vec::with_capacity(segments + 2);
    points.push(egui::Pos2::new(cx - half_w, cy));
    for i in 0..segments {
        let x = cx - half_w + (i as f32 + 0.5) * seg_w;
        let y = if i % 2 == 0 { cy - h } else { cy + h };
        points.push(egui::Pos2::new(x, y));
    }
    points.push(egui::Pos2::new(cx + half_w, cy));
    for pair in points.windows(2) {
        painter.line_segment([pair[0], pair[1]], stroke);
    }

    // Lead-out wire (right)
    let end_x = cx + half_w + seg_w;
    painter.line_segment(
        [egui::Pos2::new(cx + half_w, cy), egui::Pos2::new(end_x, cy)],
        stroke,
    );
}

/// Draw a capacitor symbol (two parallel plates) inside the given rectangle.
fn draw_capacitor(painter: &egui::Painter, rect: egui::Rect, theme: &DiagramTheme) {
    let stroke = egui::Stroke::new(theme.symbol_stroke_width, theme.symbol_color);
    let cx = rect.center().x;
    let cy = rect.center().y;
    let plate_h = rect.height() * 0.6;
    let gap = rect.width() * 0.10;

    // Left wire
    painter.line_segment(
        [egui::Pos2::new(rect.left() + 4.0, cy), egui::Pos2::new(cx - gap, cy)],
        stroke,
    );
    // Left plate
    painter.line_segment(
        [egui::Pos2::new(cx - gap, cy - plate_h / 2.0), egui::Pos2::new(cx - gap, cy + plate_h / 2.0)],
        egui::Stroke::new(theme.symbol_stroke_width + 1.0, theme.symbol_color),
    );
    // Right plate
    painter.line_segment(
        [egui::Pos2::new(cx + gap, cy - plate_h / 2.0), egui::Pos2::new(cx + gap, cy + plate_h / 2.0)],
        egui::Stroke::new(theme.symbol_stroke_width + 1.0, theme.symbol_color),
    );
    // Right wire
    painter.line_segment(
        [egui::Pos2::new(cx + gap, cy), egui::Pos2::new(rect.right() - 4.0, cy)],
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
    /// Reference to egui context for input checking.
    ctx: egui::Context,
}

impl<'a> SnarlViewer<DiagramNode> for DiagramViewer<'a> {

    fn title(&mut self, node: &DiagramNode) -> String {
        node.title().to_string()
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

        if self.schematic_mode {
             // Hide labels in schematic mode to keep dots centered and clean
        }

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

        if self.schematic_mode {
             // Hide labels in schematic mode
        }

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
        
        let body_height = self.theme.body_min_size.y;
        let symbol_size = self.theme.body_min_size.x;

        // Use fixed width based on theme to prevent infinite expansion feedback loop
        ui.vertical_centered(|ui| {
            ui.set_width(symbol_size);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(symbol_size, body_height), egui::Sense::hover());
            let painter = ui.painter();

            // Draw schematic symbol
            draw_symbol_v2(painter, rect, node, self.theme);

            // Draw instance label
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(instance_name)
                    .size(11.0)
                    .color(egui::Color32::from_rgb(180, 180, 190)),
            );
        });
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
        egui::Frame {
            fill: self.theme.node_bg,
            stroke: self.theme.node_stroke,
            inner_margin: egui::Margin::same(0),
            corner_radius: egui::CornerRadius::same(self.theme.node_rounding),
            ..Default::default()
        }
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
                 println!("[DEBUG] has_graph_menu mapped SCREEN {:?} to GRAPH: {:?}", pos, self.last_click_pos);
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
    for (_sid, pos, snarl_node) in snarl.nodes_pos_ids() {
        let DiagramNode::Component { id, .. } = snarl_node;
        if let Some(diagram_node) = diagram.get_node_mut(*id) {
            diagram_node.position = egui::Pos2::new(pos.x, pos.y);
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
        {
            let dirty = world.get_resource::<WorkbenchState>()
                .map(|s| s.diagram_dirty)
                .unwrap_or(false);
            if dirty {
                if let Some(state) = world.get_resource::<WorkbenchState>() {
                    if let Some(ref model) = state.open_model {
                        let source = model.source.clone();
                        let pool = bevy::tasks::AsyncComputeTaskPool::get();
                        let task = pool.spawn(async move {
                            import_model_to_diagram(&source)
                        });
                        if let Some(mut ds) = world.get_resource_mut::<DiagramState>() {
                            ds.parse_task = Some(task);
                        }
                    }
                }
                if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
                    state.diagram_dirty = false;
                }
            }
        }

        // ── Poll import task ──
        let mut is_parsing = false;
        if let Some(mut ds) = world.get_resource_mut::<DiagramState>() {
            if let Some(mut task) = ds.parse_task.take() {
                if let Some(diagram_opt) = futures_lite::future::block_on(futures_lite::future::poll_once(&mut task)) {
                    if let Some(diagram) = diagram_opt {
                        ds.diagram = diagram;
                        ds.rebuild_snarl();
                    }
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

        // ── Breadcrumb bar ──
        let (has_model, display_name, is_read_only, has_back) = {
            let state = world.get_resource::<WorkbenchState>();
            state.map(|s| {
                s.open_model.as_ref().map(|m| {
                    (true, m.display_name.clone(), m.read_only, !s.navigation_stack.is_empty())
                }).unwrap_or((false, String::new(), false, false))
            }).unwrap_or((false, String::new(), false, false))
        };

        if has_model {
            ui.horizontal(|ui| {
                // Read-only badge
                if is_read_only {
                    ui.colored_label(egui::Color32::from_rgb(200, 150, 50), "👁 Read-only");
                } else {
                    ui.colored_label(egui::Color32::GREEN, "✏️ Editing");
                }
                ui.label(format!("• {}", display_name));

                // Back button
                if has_back {
                    if ui.small_button("← Back").clicked() {
                        if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
                            if let Some(prev_path) = state.navigation_stack.pop() {
                                state.open_model = None;
                                state.diagram_dirty = true;
                                let _ = prev_path;
                            }
                        }
                    }
                }
            });
            ui.separator();
        }

        // ── Toolbar ──
        ui.horizontal(|ui| {
            // Mode toggle
            {
                let schematic = world.get_resource::<DiagramState>()
                    .map(|ds| ds.schematic_mode)
                    .unwrap_or(true);
                let label = if schematic { "🖼 Schematic" } else { "📊 NodeGraph" };
                if ui.selectable_label(schematic, label).clicked() {
                    if let Some(mut ds) = world.get_resource_mut::<DiagramState>() {
                        ds.schematic_mode = !ds.schematic_mode;
                    }
                }
            }
            // Bundle Examples
            if ui.button("📁 Load RC Example").clicked() {
                auto_place_rc_circuit(world);
            }
            ui.separator();

            // Compile & Run
            if ui.button("🚀 COMPILE & RUN").clicked() {
                // handled below
                ui.memory_mut(|m| m.data.insert_temp(egui::Id::new("diagram_compile"), true));
            }
            ui.separator();

            // Stats
            {
                let s = world.get_resource::<DiagramState>();
                if let Some(st) = s {
                    ui.label(
                        egui::RichText::new(format!("{} components · {} wires", st.diagram.nodes.len(), st.diagram.edges.len()))
                            .size(10.0)
                            .color(egui::Color32::from_rgb(160, 160, 170)),
                    );
                    if let Some(status) = &st.compile_status {
                        ui.separator();
                        let color = if st.compile_ok { egui::Color32::GREEN } else { egui::Color32::LIGHT_RED };
                        ui.colored_label(color, status);
                    }
                }
            }
            ui.separator();

            // Clear
            if ui.small_button("🗑 Clear").clicked() {
                if let Some(mut s) = world.get_resource_mut::<DiagramState>() {
                    s.diagram = VisualDiagram::default();
                    s.snarl = Snarl::default();
                    s.compile_status = None;
                }
            }
        });
        ui.separator();


        // ── Canvas (egui-snarl) ──
        ui.set_min_height(600.0);
        // Read theme + schematic mode
        let theme = world.get_resource::<DiagramTheme>()
            .cloned()
            .unwrap_or_default();
        let schematic_mode = world.get_resource::<DiagramState>()
            .map(|ds| ds.schematic_mode)
            .unwrap_or(true);

        // Build custom snarl style
        let mut snarl_style = SnarlStyle::default();
        snarl_style.pin_size = Some(theme.port_dot_radius * 2.0);
        snarl_style.collapsible = Some(false);
        snarl_style.header_drag_space = Some(egui::vec2(0.0, 0.0));

        // ── Canvas (egui-snarl) ──
        let available_size = ui.available_size().max(egui::vec2(100.0, 500.0));
        let (rect, _) = ui.allocate_exact_size(available_size, egui::Sense::hover());

        if let Some(mut ds) = world.get_resource_mut::<DiagramState>() {
            let mut viewer = DiagramViewer {
                schematic_mode,
                theme: &theme,
                canvas_rect: rect,
                graph_viewport: None,
                last_click_pos: ds.last_click_pos,
                ctx: ui.ctx().clone(),
            };

            // Render snarl inside the allocated rect using a stable child UI
            // The rect.min must match the panel's start point for correct coordinate mapping
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |child_ui| {
                child_ui.set_clip_rect(rect);
                ds.snarl.show(&mut viewer, &snarl_style, "diagram", child_ui);
            });

            // Save back the click position if it was updated (e.g. in has_graph_menu)
            ds.last_click_pos = viewer.last_click_pos;

            // Sync
            let DiagramState { snarl, diagram, .. } = &mut *ds;
            sync_connections(snarl, diagram);
        }

        // ── Compile (deferred check) ──
        let compile_clicked = ui.memory(|m| {
            m.data.get_temp::<bool>(egui::Id::new("diagram_compile")).unwrap_or(false)
        });
        if compile_clicked {
            ui.memory_mut(|m| m.data.insert_temp(egui::Id::new("diagram_compile"), false));
            do_compile(world);
        }
    }
}

/// Execute the compile-and-run workflow: generate source → write temp file
/// → spawn entity → send compile command.
fn do_compile(world: &mut World) {
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
            is_stepping: true,
        },
    )).id();

    // Checkpoint the source into the Document registry before sending
    // the Compile command — registry is the canonical source.
    {
        let mut registry = world.resource_mut::<crate::ui::ModelicaDocumentRegistry>();
        registry.checkpoint_source(entity, source.clone());
    }

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

/// Auto-place a classic RC circuit (Voltage -> Resistor -> Capacitor -> Ground).
fn auto_place_rc_circuit(world: &mut World) {
    let lib = msl_component_library();
    
    // Define layout
    let components = [
        ("Modelica.Electrical.Analog.Sources.ConstantVoltage", egui::pos2(-200.0, 0.0)),
        ("Modelica.Electrical.Analog.Basic.Resistor", egui::pos2(0.0, -100.0)),
        ("Modelica.Electrical.Analog.Basic.Capacitor", egui::pos2(200.0, 0.0)),
        ("Modelica.Electrical.Analog.Basic.Ground", egui::pos2(0.0, 100.0)),
    ];

    for (msl_path, pos) in components {
        if let Some(def) = lib.iter().find(|c| c.msl_path == msl_path) {
             if let Some(mut state) = world.get_resource_mut::<DiagramState>() {
                 state.add_component(def.clone(), pos);
             }
        }
    }

    // Auto-connect
    if let Some(mut ds) = world.get_resource_mut::<DiagramState>() {
        // CVoltage.p -> Resistor.p
        // Resistor.n -> Capacitor.p
        // Capacitor.n -> Ground.p
        // Ground.p -> CVoltage.n
        
         let nodes = ds.diagram.nodes.clone();
         if nodes.len() >= 4 {
             let v = nodes.iter().find(|n| n.component_def.name == "ConstantVoltage").map(|n| n.id);
             let r = nodes.iter().find(|n| n.component_def.name == "Resistor").map(|n| n.id);
             let c = nodes.iter().find(|n| n.component_def.name == "Capacitor").map(|n| n.id);
             let g = nodes.iter().find(|n| n.component_def.name == "Ground").map(|n| n.id);

             if let (Some(vid), Some(rid), Some(cid), Some(gid)) = (v, r, c, g) {
                 ds.diagram.add_edge(vid, "p".to_string(), rid, "p".to_string());
                 ds.diagram.add_edge(rid, "n".to_string(), cid, "p".to_string());
                 ds.diagram.add_edge(cid, "n".to_string(), gid, "p".to_string());
                 ds.diagram.add_edge(gid, "p".to_string(), vid, "n".to_string());
                 
                 // Rebuild snarl to show connections
                 ds.rebuild_snarl();
             }
         }
    }
}
 
