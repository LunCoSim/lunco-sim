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
use crate::ui::theme::ModelicaThemeExt;
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

/// Theme-derived colour snapshot consumed by every layer inside the
/// canvas this frame. Stashed in the egui context's data cache (by
/// type) at the entry of [`CanvasDiagramPanel::render`] so the
/// [`NodeVisual`] / [`EdgeVisual`] trait objects — which have no
/// `World` access — can still pick theme-aware colours on draw.
///
/// Recomputed each frame; cloning is a handful of `Color32` copies.
#[derive(Clone, Debug)]
pub struct CanvasThemeSnapshot {
    pub card_fill: egui::Color32,
    pub node_label: egui::Color32,
    pub type_label: egui::Color32,
    pub port_fill: egui::Color32,
    pub port_stroke: egui::Color32,
    pub select_stroke: egui::Color32,
    pub inactive_stroke: egui::Color32,
    pub icon_only_stroke: egui::Color32,
}

impl CanvasThemeSnapshot {
    pub fn from_theme(theme: &lunco_theme::Theme) -> Self {
        let c = &theme.colors;
        let t = &theme.tokens;
        let s = &theme.schematic;
        Self {
            // Card background tuned to contrast cleanly with the
            // blue-heavy MSL icon palette (Modelica Blocks / many
            // Electrical components use strong blues). Delegates to
            // the theme's dedicated `canvas_card` schematic token —
            // see `lunco_theme::SchematicTokens::canvas_card`.
            card_fill: s.canvas_card,
            node_label: t.text,
            type_label: t.text_subdued,
            port_fill: c.overlay1,
            port_stroke: c.surface2,
            // (c still referenced below for ports/selection; keep)
            // Selection follows `tokens.accent` so the active-icon
            // ring matches the rest of the app's accent chrome.
            select_stroke: t.accent,
            // Idle border: muted edge, same intent as the faint
            // outline around any inactive widget.
            inactive_stroke: c.overlay0,
            // Icon-only ring uses `warning` — signals "this is
            // decorative, doesn't carry connectors" via the same
            // colour the app uses for other cautionary chrome.
            icon_only_stroke: t.warning,
        }
    }
}

/// Fetch the theme snapshot stored for this frame by the canvas
/// render entry. `None` when the canvas is rendered outside our
/// panel (tests / demos); caller falls back to a default snapshot
/// derived from `Theme::dark()`.
fn canvas_theme_from_ctx(ctx: &egui::Context) -> CanvasThemeSnapshot {
    let id = egui::Id::new("lunco.modelica.canvas_theme_snapshot");
    ctx.data(|d| d.get_temp::<CanvasThemeSnapshot>(id))
        .unwrap_or_else(|| {
            CanvasThemeSnapshot::from_theme(&lunco_theme::Theme::dark())
        })
}

/// Build the generic `lunco_canvas` layer theme (grid, selection halo,
/// tool preview, zoom-bar overlay) from the active LunCoSim theme.
/// Pushed to the canvas each frame so its built-in layers render in
/// palette-matched colours instead of their hardcoded dark defaults.
fn layer_theme_from(theme: &lunco_theme::Theme) -> lunco_canvas::CanvasLayerTheme {
    let c = &theme.colors;
    let t = &theme.tokens;
    // Grid: dim overlay dot. Using overlay0 at low alpha reads on
    // both Mocha (dark) and Latte (light) without competing with
    // diagram content.
    let grid = {
        let g = c.overlay0;
        egui::Color32::from_rgba_unmultiplied(g.r(), g.g(), g.b(), 60)
    };
    let rubber_fill = {
        let a = t.accent;
        egui::Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 40)
    };
    let shadow = {
        let b = c.base;
        egui::Color32::from_rgba_unmultiplied(b.r(), b.g(), b.b(), 110)
    };
    lunco_canvas::CanvasLayerTheme {
        grid,
        selection_outline: t.accent,
        ghost_edge: t.accent,
        snap_target: t.success,
        rubber_band_fill: rubber_fill,
        rubber_band_stroke: t.accent,
        overlay_fill: c.surface0,
        overlay_stroke: c.surface2,
        overlay_shadow: shadow,
        overlay_text: t.text,
    }
}

/// Store a theme snapshot in the egui data cache under a well-known
/// id. Counterpart to [`canvas_theme_from_ctx`].
fn store_canvas_theme(ctx: &egui::Context, snap: CanvasThemeSnapshot) {
    let id = egui::Id::new("lunco.modelica.canvas_theme_snapshot");
    ctx.data_mut(|d| d.insert_temp(id, snap));
}

/// Per-component icon visual. Renders, in priority order:
///
/// 1. The class's decoded `Icon(graphics={...})` annotation, if the
///    projector extracted one (user-defined classes from the open
///    document). Painted via [`crate::icon_paint::paint_graphics`].
/// 2. The pre-rasterised SVG icon if `icon_asset` resolved (MSL
///    components that ship with the palette).
/// 3. A stylised rounded-rectangle fallback with the type label.
///
/// Ports render as filled dots on the icon boundary in all cases.
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
    /// `expandable connector` class (MLS §9.1.3). Rendered with a
    /// dashed border in an accent colour so users can distinguish
    /// them from regular connectors — expandable connectors collect
    /// variables across connections dynamically and have different
    /// semantics.
    expandable_connector: bool,
    /// Decoded graphics from the class's `Icon` annotation. When
    /// present, takes precedence over the SVG icon path so user
    /// classes show their authored graphics instead of falling back
    /// to a generic placeholder.
    icon_graphics: Option<crate::annotations::Icon>,
    /// Per-instance rotation (degrees CCW, Modelica frame) applied to
    /// the icon body itself — rotates both the SVG raster and the
    /// `paint_graphics` primitives uniformly. Without this, mirror /
    /// rotated MSL placements showed correct port positions but a
    /// wrong-looking body.
    rotation_deg: f32,
    /// Mirror flags applied to the icon body, before rotation
    /// (MLS Annex D).
    mirror_x: bool,
    mirror_y: bool,
    /// Instance name this component is drawn for — "R1", "C1", …
    /// Drives the `%name` substitution in authored `Text` primitives
    /// (Modelica's convention for showing the instance label on the
    /// icon body). Empty when the projector didn't provide one.
    instance_name: String,
    /// Class name (leaf — e.g. "Resistor"). Drives `%class`
    /// substitution in authored `Text` primitives.
    class_name: String,
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
        let theme_snap = canvas_theme_from_ctx(ctx.ui.ctx());

        // Always paint a solid card background *underneath* the SVG.
        // Why: MSL icons are outlined shapes — the SVG pixels inside
        // the outline are transparent by design, so without a bg the
        // connection lines running behind an icon are visible through
        // its body. That reads as "the diagram is a sheet of glass"
        // rather than "icons are opaque tiles." Dymola/OMEdit both
        // paint each icon on its own opaque card for the same reason.
        painter.rect_filled(rect, 6.0, theme_snap.card_fill);

        // Priority 1: authored graphics from the class's `Icon`
        // annotation. Beats the SVG path so user-defined classes show
        // their own primitives even when no pre-rasterised icon
        // exists for them. Per-instance orientation rotates+mirrors
        // every primitive at the rect level so placement-rotation
        // shows visually, not just on the port positions.
        let orientation = crate::icon_paint::IconOrientation {
            rotation_deg: self.rotation_deg,
            mirror_x: self.mirror_x,
            mirror_y: self.mirror_y,
        };
        let mut drew_svg = false;
        if let Some(icon) = &self.icon_graphics {
            let sub = crate::icon_paint::TextSubstitution {
                name: (!self.instance_name.is_empty()).then_some(self.instance_name.as_str()),
                class_name: (!self.class_name.is_empty()).then_some(self.class_name.as_str()),
            };
            crate::icon_paint::paint_graphics_full(
                painter,
                rect,
                icon.coordinate_system,
                orientation,
                Some(&sub),
                &icon.graphics,
            );
            drew_svg = true;
        }
        if !drew_svg && !self.icon_asset.is_empty() {
            if let Some(bytes) = svg_bytes_for(&self.icon_asset) {
                let svg_orient = super::svg_renderer::SvgOrientation {
                    rotation_deg: self.rotation_deg,
                    mirror_x: self.mirror_x,
                    mirror_y: self.mirror_y,
                };
                super::svg_renderer::draw_svg_to_egui_oriented(
                    painter,
                    rect,
                    &bytes,
                    svg_orient,
                );
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
                    theme_snap.type_label,
                );
            }
        }

        // Selection outline draws ON TOP of the icon so it's always
        // visible even over busy SVG content. Icon-only classes
        // (no connectors, visual-only) get a dashed border instead
        // of solid — a signal that the component isn't hookable.
        let stroke = if selected {
            egui::Stroke::new(2.0, theme_snap.select_stroke)
        } else if self.icon_only {
            egui::Stroke::new(1.0, theme_snap.icon_only_stroke)
        } else if self.expandable_connector {
            // Accent colour (same family as the select stroke) so the
            // dashed border is visually distinct from icon-only.
            egui::Stroke::new(1.5, theme_snap.select_stroke)
        } else {
            egui::Stroke::new(1.0, theme_snap.inactive_stroke)
        };
        let wants_dashed = (self.icon_only || self.expandable_connector) && !selected;
        if wants_dashed {
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
                theme_snap.node_label,
            );
        }

        // Ports — shape per connector causality (OMEdit / Dymola
        // convention):
        //   • input  → filled square   (RealInput, BooleanInput, …)
        //   • output → filled triangle pointing outward
        //   • acausal physical → filled circle (Pin, Flange, HeatPort, …)
        // Direction is derived from where the port sits on the icon
        // boundary, classified the same way edges classify port_dir.
        for port in &node.ports {
            let world = CanvasPos::new(
                node.rect.min.x + port.local_offset.x,
                node.rect.min.y + port.local_offset.y,
            );
            let p = ctx.viewport.world_to_screen(world, ctx.screen_rect);
            let center = egui::pos2(p.x, p.y);

            // Port shape from AST-derived kind (the projector wrote
            // `"input"` / `"output"` / `"acausal"` into `port.kind`;
            // see the `CanvasPort` construction in `project_scene`).
            let shape = match port.kind.as_str() {
                "input" => PortShape::InputSquare,
                "output" => PortShape::OutputTriangle,
                _ => PortShape::AcausalCircle,
            };

            // Outward direction in *screen* space — derived from
            // which icon edge the port sits closest to.
            let cx = node.rect.min.x + node.rect.width() * 0.5;
            let cy = node.rect.min.y + node.rect.height() * 0.5;
            let dir = port_edge_dir(world.x - cx, world.y - cy);

            let fill = theme_snap.port_fill;
            let stroke = egui::Stroke::new(1.0, theme_snap.port_stroke);
            paint_port_shape(painter, center, shape, dir, fill, stroke);
        }

        // Hover tooltip. The canvas claims the whole widget rect
        // with `Sense::click_and_drag()` so `ui.interact(.., Sense::hover())`
        // and even `show_tooltip_at_pointer` get suppressed at the
        // visual's layer. Paint the tooltip card directly with the
        // foreground painter — bypasses egui's interaction layering
        // entirely.
        let cursor = ctx.ui.ctx().pointer_hover_pos();
        // Suppress the tooltip when the cursor isn't actually over
        // the canvas (e.g. floated past the widget edge while still
        // hovering the icon's *world rect*). Without this the card
        // can sit on top of the side panels because it paints in
        // an unclipped layer.
        let canvas_widget_rect = ctx.ui.max_rect();
        let in_canvas = cursor
            .map(|c| canvas_widget_rect.contains(c))
            .unwrap_or(false);
        let is_hovered = cursor
            .map(|c| rect.contains(c))
            .unwrap_or(false)
            && in_canvas;
        if is_hovered && !self.instance_name.is_empty() {
            let cursor = cursor.unwrap();
            let snap =
                lunco_viz::kinds::canvas_plot_node::fetch_node_state(
                    ctx.ui.ctx(),
                );
            let prefix = format!("{}.", self.instance_name);
            let mut rows: Vec<(&String, &f64)> = snap
                .values
                .iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .collect();
            rows.sort_by(|a, b| a.0.cmp(b.0));
            paint_hover_card(
                ctx.ui,
                cursor,
                &self.instance_name,
                &self.class_name,
                &rows,
            );
        }
    }
    fn debug_name(&self) -> &str {
        "modelica.icon"
    }
}

/// Direct-paint hover card (foreground layer). Used because the
/// canvas's `Sense::click_and_drag()` swallows ordinary tooltip
/// hooks at the visual layer.
fn paint_hover_card(
    ui: &mut egui::Ui,
    cursor: egui::Pos2,
    instance: &str,
    class_name: &str,
    rows: &[(&String, &f64)],
) {
    let theme = lunco_canvas::theme::current(ui.ctx());
    let layer_id = egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new(("modelica_icon_hover_card", instance)),
    );
    let painter = ui.ctx().layer_painter(layer_id);
    // Clip to the canvas widget rect so the card never paints over
    // the side panels (the user would otherwise see a tooltip
    // ghost overlapping the Twin Browser when hovering an icon
    // near the canvas's left edge).
    let canvas_clip = ui.max_rect();
    let painter = painter.with_clip_rect(canvas_clip);

    // Build text lines first so we can size the card accordingly.
    let mut lines: Vec<(String, bool)> = Vec::with_capacity(rows.len() + 4);
    lines.push((instance.to_string(), true));
    if !class_name.is_empty() {
        lines.push((class_name.to_string(), false));
    }
    if rows.is_empty() {
        lines.push(("(no values yet — run a sim)".to_string(), false));
    } else {
        for (k, v) in rows {
            let short = k.strip_prefix(&format!("{instance}.")).unwrap_or(k);
            lines.push((format!("{short:<10}  {v:>10.4}"), false));
        }
    }

    let line_h = 14.0_f32;
    let pad = 6.0_f32;
    // Estimate width: 7 px per char (monospace). egui doesn't expose
    // `Painter::text_size` cheaply; this is plenty for the typical
    // path widths we render.
    let text_w = lines
        .iter()
        .map(|(s, _)| s.chars().count() as f32 * 7.0)
        .fold(0.0_f32, f32::max);
    let card_w = (text_w + pad * 2.0).clamp(120.0, 360.0);
    let card_h = lines.len() as f32 * line_h + pad * 2.0;

    // Anchor card to the right of the cursor with a small offset;
    // flip to the left if we'd run off the screen edge.
    let screen = ui.ctx().screen_rect();
    let mut origin =
        egui::pos2(cursor.x + 14.0, cursor.y + 14.0);
    if origin.x + card_w > screen.max.x {
        origin.x = cursor.x - card_w - 14.0;
    }
    if origin.y + card_h > screen.max.y {
        origin.y = cursor.y - card_h - 14.0;
    }
    let card_rect = egui::Rect::from_min_size(
        origin,
        egui::vec2(card_w, card_h),
    );
    // Drop shadow so the card pops over the diagram.
    painter.rect_filled(
        card_rect.translate(egui::vec2(0.0, 2.0)),
        6.0,
        theme.overlay_shadow,
    );
    painter.rect_filled(card_rect, 6.0, theme.overlay_fill);
    painter.rect_stroke(
        card_rect,
        6.0,
        egui::Stroke::new(1.0, theme.overlay_stroke),
        egui::StrokeKind::Outside,
    );

    let mut y = origin.y + pad;
    for (line, is_title) in &lines {
        let font = if *is_title {
            egui::FontId::proportional(13.0)
        } else {
            egui::FontId::monospace(11.0)
        };
        let color = if *is_title {
            theme.overlay_text
        } else {
            theme.overlay_text.gamma_multiply(0.85)
        };
        painter.text(
            egui::pos2(origin.x + pad, y),
            egui::Align2::LEFT_TOP,
            line,
            font,
            color,
        );
        y += line_h;
    }
}

/// Paint a chain of small bright dots along a polyline that march
/// from the first to the last vertex at constant screen-pixel speed.
/// Phase keyed off wall-clock `time` so all wires stay in sync.
/// Used as the "this connection is alive" overlay during simulation
/// — Simulink/SPICE-style, no per-edge flow data needed yet.
fn paint_flow_dots(
    painter: &egui::Painter,
    polyline: &[egui::Pos2],
    base_color: egui::Color32,
    time: f64,
) {
    if polyline.len() < 2 {
        return;
    }
    let mut total_len = 0.0_f32;
    for w in polyline.windows(2) {
        total_len += (w[1] - w[0]).length();
    }
    if total_len < 1.0 {
        return;
    }
    // Spacing + speed in screen pixels. Tunable.
    const SPACING_PX: f32 = 28.0;
    const SPEED_PX_S: f32 = 36.0;
    let phase = ((time as f32) * SPEED_PX_S).rem_euclid(SPACING_PX);
    let dot_color = egui::Color32::from_rgba_unmultiplied(
        base_color.r(),
        base_color.g(),
        base_color.b(),
        220,
    );
    let mut s = phase;
    while s < total_len {
        // Walk the polyline to find the segment containing arc-length s.
        let mut acc = 0.0_f32;
        for w in polyline.windows(2) {
            let seg_len = (w[1] - w[0]).length();
            if s <= acc + seg_len {
                let t = ((s - acc) / seg_len).clamp(0.0, 1.0);
                let p = w[0] + (w[1] - w[0]) * t;
                painter.circle_filled(p, 2.2, dot_color);
                break;
            }
            acc += seg_len;
        }
        s += SPACING_PX;
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
/// Which edge of the icon a port sits on. Determines which axis the
/// wire's first segment ("stub") runs along — Dymola/OMEdit wire
/// pretty-routing convention. Modelica port placement is in (-100..100)
/// per axis; we classify by which extreme the port sits closest to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortDir {
    Left,
    Right,
    Up,
    Down,
    /// Port sits in the interior of the icon (or no info). Routing
    /// degrades to plain Z-bend.
    None,
}

impl PortDir {
    fn as_str(self) -> &'static str {
        match self {
            PortDir::Left => "left",
            PortDir::Right => "right",
            PortDir::Up => "up",
            PortDir::Down => "down",
            PortDir::None => "",
        }
    }
    fn from_str(s: &str) -> PortDir {
        match s {
            "left" => PortDir::Left,
            "right" => PortDir::Right,
            "up" => PortDir::Up,
            "down" => PortDir::Down,
            _ => PortDir::None,
        }
    }
    /// Unit vector pointing *outward* from the icon at this edge,
    /// in screen coordinates (+Y down). Used to extend the wire
    /// stub away from the icon body.
    fn outward(self) -> (f32, f32) {
        match self {
            PortDir::Left => (-1.0, 0.0),
            PortDir::Right => (1.0, 0.0),
            PortDir::Up => (0.0, -1.0),
            PortDir::Down => (0.0, 1.0),
            PortDir::None => (0.0, 0.0),
        }
    }
}

// `rotate_modelica_point` / `rotate_local_point` / `mirror_local_point`
// retired — replaced by [`crate::icon_transform::IconTransform`], which
// folds mirror + rotate + scale + Y-flip into a single matrix that the
// projector applies via `apply` / `apply_dir`. See
// `crates/lunco-modelica/src/icon_transform.rs`.

/// Classify a 2D direction into one of the four cardinal icon edges,
/// in **screen frame** (+X right, +Y down — same convention as
/// [`PortDir::outward`]). Used to decide which way a wire stub
/// should extend out of a port.
///
/// The threshold makes any direction whose components are both close
/// to zero collapse to [`PortDir::None`] — Z-bend routing falls
/// through to the original midpoint logic in that case.
fn port_edge_dir(x: f32, y: f32) -> PortDir {
    let threshold = 0.4;
    let ax = x.abs();
    let ay = y.abs();
    if ax < threshold && ay < threshold {
        return PortDir::None;
    }
    if ax >= ay {
        if x >= 0.0 { PortDir::Right } else { PortDir::Left }
    } else if y >= 0.0 {
        // +Y down in screen → bottom edge of icon.
        PortDir::Down
    } else {
        PortDir::Up
    }
}

/// Map a Modelica connector type's leaf name to a wire colour.
/// Colour choices follow Dymola/OMEdit conventions so users coming
/// from those tools get instant domain recognition. Unknown types
/// fall back to a neutral grey-blue.
fn wire_color_for(connector_type: &str) -> egui::Color32 {
    let leaf = connector_type
        .rsplit('.')
        .next()
        .unwrap_or(connector_type);
    use egui::Color32 as C;
    match leaf {
        // Electrical: blue family (Pin, PositivePin, NegativePin, Plug)
        "Pin" | "PositivePin" | "NegativePin" | "Plug" | "PositivePlug"
        | "NegativePlug" => C::from_rgb(60, 120, 200),
        // Translational + rotational mechanics: brown
        "Flange_a" | "Flange_b" | "Flange" | "Support" => {
            C::from_rgb(140, 90, 50)
        }
        // Heat transfer: orange/red
        "HeatPort_a" | "HeatPort_b" | "HeatPort" => C::from_rgb(210, 110, 50),
        // Fluid: green/blue
        "FluidPort" | "FluidPort_a" | "FluidPort_b" => C::from_rgb(70, 160, 180),
        // Real signals: magenta
        "RealInput" | "RealOutput" => C::from_rgb(180, 70, 170),
        // Boolean signals: red
        "BooleanInput" | "BooleanOutput" => C::from_rgb(200, 60, 60),
        // Integer signals: green
        "IntegerInput" | "IntegerOutput" => C::from_rgb(70, 160, 80),
        // Frame_a/Frame_b (multibody): purple
        "Frame" | "Frame_a" | "Frame_b" => C::from_rgb(120, 80, 180),
        // Default — neutral grey-blue, distinguishable from selection
        _ => C::from_rgb(110, 130, 150),
    }
}

/// Per-edge wire visual. Carries the wire colour + the port-direction
/// hints baked in by the projector so each edge knows which axis to
/// extend before bending. Two stubs (one out of each port) followed
/// by a Z-bend gives the "wire grows out of the connector" look that
/// matches Dymola/OMEdit and reads much cleaner than the previous
/// always-x-midpoint Z.
struct OrthogonalEdgeVisual {
    color: egui::Color32,
    from_dir: PortDir,
    to_dir: PortDir,
    /// Authored / stored waypoints in **canvas world** coords
    /// (Modelica +Y is flipped to canvas +Y at projector time).
    /// When non-empty, the renderer emits a polyline through the
    /// waypoints instead of the auto Z-bend.
    waypoints_world: Vec<CanvasPos>,
    /// True when the connection is causal (output→input signal),
    /// so the renderer emits an arrowhead at the input end. False
    /// for acausal connectors (Pin, Flange, FluidPort, …) — the
    /// MLS convention is symmetric arrows-or-no-arrows for those.
    is_causal: bool,
    /// Fully-qualified source port path, e.g. `"engine.thrust"` /
    /// `"tank.fuel_out"`. Used to look up the current value (and
    /// its unit) for the hover tooltip, and for flow-animation
    /// direction (sign of `{source_path}.{flow_var}` at runtime).
    source_path: String,
    /// Fully-qualified target port path. Secondary — used only when
    /// the source path has no sampled value (e.g. inputs are only
    /// visible on the target side).
    target_path: String,
    /// Connector causality classification (Input / Output / Acausal)
    /// derived from the connector class AST at projection time.
    /// Drives arrowhead rendering and animation eligibility.
    kind: crate::visual_diagram::PortKind,
    /// Flow variables declared on the connector class (name + unit).
    /// Empty for causal signals — those never animate. Non-empty →
    /// we sample `{source_path}.{name}` to drive flow animation and
    /// to populate the hover tooltip with each variable + unit.
    flow_vars: Vec<crate::visual_diagram::FlowVarMeta>,
    /// Short class-name for the tooltip label when the connector
    /// class has no description string. Matches the MSL-style
    /// "what is this wire carrying" intuition (e.g. `"FuelPort_a"`).
    connector_leaf: String,
}

impl Default for OrthogonalEdgeVisual {
    fn default() -> Self {
        Self {
            color: wire_color_for(""),
            from_dir: PortDir::None,
            to_dir: PortDir::None,
            waypoints_world: Vec::new(),
            is_causal: false,
            source_path: String::new(),
            target_path: String::new(),
            kind: crate::visual_diagram::PortKind::Acausal,
            flow_vars: Vec::new(),
            connector_leaf: String::new(),
        }
    }
}

/// Stub length in screen pixels — long enough to clear the port dot
/// (which is itself ~4 px) and read clearly as "the wire exits the
/// port" at typical zoom levels, while still leaving room for the
/// Z-bend body. Earlier values around 10 px disappeared on default
/// auto-fit zoom; 18 stays readable across normal zoom range.
const STUB_PX: f32 = 18.0;

impl EdgeVisual for OrthogonalEdgeVisual {
    fn draw(
        &self,
        ctx: &mut DrawCtx,
        from: CanvasPos,
        to: CanvasPos,
        selected: bool,
    ) {
        let col = if selected {
            // Selection: brighten the per-type colour rather than
            // collapsing every wire to one universal "selected" blue.
            // Keeps the connector type recognisable through selection
            // chrome.
            brighten(self.color)
        } else {
            self.color
        };
        let width = if selected { 2.2 } else { 1.5 };
        let stroke = egui::Stroke::new(width, col);
        let painter = ctx.ui.painter();

        // Authored polyline: if the edge carries waypoints (from a
        // `connect(...) annotation(Line(points={{x,y},...}))` clause
        // or a user edit), emit a stub-from-port → waypoints → stub-
        // into-port polyline and skip the auto-Z router entirely.
        //
        // Optimistic fallback during drag: the waypoints are baked in
        // canvas-world coords at projection time. If the user is mid-
        // drag of one of the connected nodes, the port has moved but
        // the waypoints haven't, so the strict polyline form draws
        // an obvious zigzag back to the stale anchor. Detect that
        // (port noticeably far from the nearest authored endpoint)
        // and fall through to auto-Z so the wire visually tracks the
        // dragged node — when re-projection lands the waypoints come
        // back into alignment.
        if !self.waypoints_world.is_empty() {
            let from_screen = egui::pos2(from.x, from.y);
            let to_screen = egui::pos2(to.x, to.y);
            let way_screen: Vec<egui::Pos2> = self
                .waypoints_world
                .iter()
                .map(|p| {
                    let s = ctx
                        .viewport
                        .world_to_screen(*p, ctx.screen_rect);
                    egui::pos2(s.x, s.y)
                })
                .collect();
            // Stale-anchor guard: a wire whose first / last waypoint
            // sits far (> 60 screen px) from its current port end is
            // stale (drag in progress); use auto-Z instead.
            const STALE_PX: f32 = 60.0;
            let first_far = way_screen
                .first()
                .map(|p| (p.x - from_screen.x).hypot(p.y - from_screen.y) > STALE_PX)
                .unwrap_or(false);
            let last_far = way_screen
                .last()
                .map(|p| (p.x - to_screen.x).hypot(p.y - to_screen.y) > STALE_PX)
                .unwrap_or(false);
            if !(first_far || last_far) {
                let mut pts = Vec::with_capacity(way_screen.len() + 2);
                pts.push(from_screen);
                pts.extend(way_screen.iter().copied());
                pts.push(to_screen);
                for w in pts.windows(2) {
                    painter.line_segment([w[0], w[1]], stroke);
                }
                return;
            }
            // else: fall through to auto-Z below
        }

        // Build an orthogonal polyline using port-direction-aware
        // elbow placement. See [`route_orthogonal`] for the full case
        // analysis (parallel-aligned, parallel-opposed, perpendicular,
        // unknown). This replaces the older "always-midpoint Z" router
        // that produced wires crossing through icon bodies whenever
        // a port faced away from its peer.
        let polyline = route_orthogonal(
            egui::pos2(from.x, from.y),
            self.from_dir,
            egui::pos2(to.x, to.y),
            self.to_dir,
            STUB_PX,
        );
        for w in polyline.windows(2) {
            painter.line_segment([w[0], w[1]], stroke);
        }

        // Arrowhead at the input end on causal-only connections —
        // OMEdit/Dymola convention. Heuristic: connector type ends
        // with "Output"/"Input" → causal signal. Arrow points along
        // the last segment, AT the target port (the input side).
        if self.is_causal && polyline.len() >= 2 {
            let n = polyline.len();
            let tail = polyline[n - 2];
            let tip = polyline[n - 1];
            paint_arrowhead(painter, tail, tip, col);
        }

        // Live-flow animation: small dots moving along the polyline
        // at constant speed. Skips when no signal data is present
        // (sim never ran / paused) so a static diagram doesn't pulse
        // for no reason. Real per-edge flow magnitude/direction is a
        // follow-up — for now this just signals "this connection is
        // live."
        // Flow-dot animation is a *live status indicator* — it only
        // runs when the simulator is actively stepping AND a flow
        // variable has a non-negligible magnitude. Paused state → no
        // dots, even if the last sampled m_dot was large. No flow
        // variable on this connector (causal signals: throttle,
        // thrust, mass) → never animated.
        let sim_stepping = ctx
            .ui
            .ctx()
            .data(|d| {
                d.get_temp::<bool>(egui::Id::new("lunco_modelica_sim_stepping"))
            })
            .unwrap_or(false);
        if sim_stepping {
            let node_state =
                lunco_viz::kinds::canvas_plot_node::fetch_node_state(ctx.ui.ctx());
            const ACTIVITY_EPS: f64 = 1e-6;
            let (value, reverse_if_negative) = if let Some(fv) = self.flow_vars.first() {
                // Acausal flow connector: sample the first declared
                // flow variable by its AST name. MLS sign convention:
                // positive `flow` means mass flows INTO the connector
                // from the source's component; negative → mass moves
                // toward the source → reverse polyline so dots trail
                // the actual direction of travel.
                let key = format!("{}.{}", self.source_path, fv.name);
                (node_state.values.get(&key).copied(), true)
            } else {
                // Causal signal: value lives at the source port path.
                // Signal direction is already encoded in the polyline
                // from→to (output to input), so don't flip on sign;
                // just animate when the value is non-trivially
                // non-zero so the wire reads as "live data".
                let v = node_state
                    .values
                    .get(&self.source_path)
                    .or_else(|| node_state.values.get(&self.target_path))
                    .copied();
                (v, false)
            };
            if let Some(v) = value {
                if v.abs() > ACTIVITY_EPS {
                    if reverse_if_negative && v < 0.0 {
                        let mut rev = polyline.clone();
                        rev.reverse();
                        paint_flow_dots(painter, &rev, col, ctx.time);
                    } else {
                        paint_flow_dots(painter, &polyline, col, ctx.time);
                    }
                }
            }
        }

        // Hover tooltip — "<label>: <value> <unit>" when the pointer
        // is within HOVER_PX of any segment. Value is sampled from
        // the per-frame NodeStateSnapshot; renders nothing when
        // there's no sim (tooltip still shows the label + "n/a").
        if let Some(p) = ctx.ui.ctx().pointer_hover_pos() {
            const HOVER_PX: f32 = 8.0;
            let hit = polyline
                .windows(2)
                .any(|w| dist_point_to_segment(p, w[0], w[1]) <= HOVER_PX);
            if hit {
                let state = lunco_viz::kinds::canvas_plot_node::fetch_node_state(
                    ctx.ui.ctx(),
                );
                let text = edge_hover_text(self, &state);
                paint_wire_tooltip(painter, p, &text, col);
            }
        }
    }

    /// Hit-test the simplified path. Cheap enough to do at full
    /// fidelity on every click; refining for stubs would add cost
    /// but no detectability benefit (stubs are 10px each).
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

/// Translate `p` by `len` pixels in `dir`'s outward direction.
fn step(p: CanvasPos, dir: PortDir, len: f32) -> CanvasPos {
    let (ux, uy) = dir.outward();
    CanvasPos::new(p.x + ux * len, p.y + uy * len)
}

/// Compute an orthogonal polyline routed between two ports, in
/// **screen coords** (+Y down). The router emits a stub from each
/// port in its outward direction, then connects the stub-ends with
/// either an L-elbow (perpendicular ports) or a Z-bend (parallel /
/// unknown), choosing pivot positions that keep the wire from
/// doubling back across the icon body.
///
/// Cases (where `f`/`t` are the port-side stub endpoints):
///
/// * **Perpendicular** (one horizontal, one vertical): single
///   L-elbow at the corner aligned with each port's exit axis. No
///   Z-bend needed.
///
/// * **Parallel, opposed** (e.g. Right→Left, both helping): classic
///   Z-bend pivoted at the midpoint along the ports' shared exit
///   axis. Stubs already pointed at each other so the elbow lives
///   between them.
///
/// * **Parallel, same direction** (e.g. both Right) or **port faces
///   away from peer**: the "helping" extent is pushed past the
///   farther port + STUB so the wire wraps around instead of
///   doubling back through the source icon. Produces a U-shape when
///   both ports face the same direction.
///
/// * **One unknown direction**: defer to the known port's axis;
///   midpoint Z. Both unknown: plain horizontal-first Z.
///
/// Output always starts at `from` and ends at `to`; intermediate
/// points are inserted only when needed (no zero-length segments).
fn route_orthogonal(
    from: egui::Pos2,
    from_dir: PortDir,
    to: egui::Pos2,
    to_dir: PortDir,
    stub: f32,
) -> Vec<egui::Pos2> {
    use PortDir::*;
    let f_horiz = matches!(from_dir, Left | Right);
    let f_vert = matches!(from_dir, Up | Down);
    let t_horiz = matches!(to_dir, Left | Right);
    let t_vert = matches!(to_dir, Up | Down);

    // Stub-ends — extend each port outward by `stub` even when the
    // direction "doesn't help", so the wire is visibly attached to
    // the connector and the elbow logic below has a fixed anchor.
    let (uxf, uyf) = from_dir.outward();
    let (uxt, uyt) = to_dir.outward();
    let f_stub = if from_dir == None {
        from
    } else {
        egui::pos2(from.x + uxf * stub, from.y + uyf * stub)
    };
    let t_stub = if to_dir == None {
        to
    } else {
        egui::pos2(to.x + uxt * stub, to.y + uyt * stub)
    };

    // "Helps" = the port's outward axis carries us toward the other
    // port. When false, the elbow has to wrap around the icon to
    // avoid crossing through it.
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let from_helps = uxf * dx + uyf * dy > 0.0;
    let to_helps = uxt * (-dx) + uyt * (-dy) > 0.0;

    let mut pts: Vec<egui::Pos2> = Vec::with_capacity(6);
    pts.push(from);
    if from_dir != None {
        pts.push(f_stub);
    }

    // Decide the inner routing between f_stub and t_stub.
    if (f_horiz && t_vert) || (f_vert && t_horiz) {
        // Perpendicular → L-elbow at the corner that sits along
        // each port's exit axis. `from`-side determines which
        // ordinate the corner takes from which side.
        let corner = if f_horiz {
            egui::pos2(t_stub.x, f_stub.y)
        } else {
            egui::pos2(f_stub.x, t_stub.y)
        };
        if corner != f_stub && corner != t_stub {
            pts.push(corner);
        }
    } else if f_horiz && t_horiz {
        // Both horizontal. Pivot Y at midway between stub-ends;
        // pivot X at midway between stub-ends if both helping
        // (classic Z), else push past the trailing port + stub
        // so the wire wraps around instead of crossing the icon.
        let pivot_x = if from_helps && to_helps {
            (f_stub.x + t_stub.x) * 0.5
        } else if !from_helps {
            // from-stub points the wrong way — push pivot past
            // from-stub in its outward direction.
            f_stub.x
        } else {
            t_stub.x
        };
        let pivot_y = (f_stub.y + t_stub.y) * 0.5;
        pts.push(egui::pos2(pivot_x, f_stub.y));
        if (pivot_y - f_stub.y).abs() > 0.5 {
            pts.push(egui::pos2(pivot_x, pivot_y));
            pts.push(egui::pos2(t_stub.x, pivot_y));
        } else {
            pts.push(egui::pos2(t_stub.x, f_stub.y));
        }
    } else if f_vert && t_vert {
        // Mirror of the both-horizontal case.
        let pivot_y = if from_helps && to_helps {
            (f_stub.y + t_stub.y) * 0.5
        } else if !from_helps {
            f_stub.y
        } else {
            t_stub.y
        };
        let pivot_x = (f_stub.x + t_stub.x) * 0.5;
        pts.push(egui::pos2(f_stub.x, pivot_y));
        if (pivot_x - f_stub.x).abs() > 0.5 {
            pts.push(egui::pos2(pivot_x, pivot_y));
            pts.push(egui::pos2(pivot_x, t_stub.y));
        } else {
            pts.push(egui::pos2(f_stub.x, t_stub.y));
        }
    } else {
        // At least one direction unknown. Defer to whichever side
        // has a known direction; if both unknown, pick horizontal-
        // first Z-bend.
        let horizontal_first = f_horiz || t_horiz || (!f_vert && !t_vert);
        if horizontal_first {
            let midx = (f_stub.x + t_stub.x) * 0.5;
            pts.push(egui::pos2(midx, f_stub.y));
            pts.push(egui::pos2(midx, t_stub.y));
        } else {
            let midy = (f_stub.y + t_stub.y) * 0.5;
            pts.push(egui::pos2(f_stub.x, midy));
            pts.push(egui::pos2(t_stub.x, midy));
        }
    }

    if to_dir != None {
        pts.push(t_stub);
    }
    pts.push(to);

    // De-dup adjacent identical points (collinear cases above can
    // produce degenerate runs); a polyline with zero-length segments
    // confuses both the renderer and the flow-dot animator.
    pts.dedup_by(|a, b| (a.x - b.x).abs() < 0.5 && (a.y - b.y).abs() < 0.5);
    pts
}

/// Serialise a [`PortKind`](crate::visual_diagram::PortKind) into the
/// short string used in edge JSON data, so the factory can round-trip
/// it without pulling in serde enum tagging.
fn port_kind_str(kind: crate::visual_diagram::PortKind) -> &'static str {
    match kind {
        crate::visual_diagram::PortKind::Input => "input",
        crate::visual_diagram::PortKind::Output => "output",
        crate::visual_diagram::PortKind::Acausal => "acausal",
    }
}

/// Build the wire hover tooltip text from AST-derived semantics —
/// header = connector class short-name; one line per declared flow
/// variable (name = value unit) for acausal connectors; otherwise
/// one line for the source-port value itself (causal signals).
/// Formats "n/a" for variables the sim hasn't sampled yet.
fn edge_hover_text(
    edge: &OrthogonalEdgeVisual,
    state: &lunco_viz::kinds::canvas_plot_node::NodeStateSnapshot,
) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = write!(&mut out, "{}", edge.connector_leaf);
    if edge.flow_vars.is_empty() {
        let v = state
            .values
            .get(&edge.source_path)
            .or_else(|| state.values.get(&edge.target_path))
            .copied();
        let value_str = match v {
            Some(v) => format!("{v:.3}"),
            None => "n/a".into(),
        };
        let _ = write!(&mut out, "\n  value = {value_str}");
    } else {
        for fv in &edge.flow_vars {
            let key = format!("{}.{}", edge.source_path, fv.name);
            let v = state.values.get(&key).copied();
            let value_str = match v {
                Some(v) => format!("{v:.3}"),
                None => "n/a".into(),
            };
            let unit = if fv.unit.is_empty() {
                String::new()
            } else {
                format!(" {}", fv.unit)
            };
            let _ = write!(&mut out, "\n  {} = {value_str}{unit}", fv.name);
        }
    }
    out
}

/// Perpendicular distance from point `p` to segment `a`→`b`, in
/// screen pixels. Used for hit-testing wire hover.
fn dist_point_to_segment(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let ap = p - a;
    let len_sq = ab.x * ab.x + ab.y * ab.y;
    if len_sq < 1e-6 {
        return (p - a).length();
    }
    let t = ((ap.x * ab.x + ap.y * ab.y) / len_sq).clamp(0.0, 1.0);
    let proj = egui::pos2(a.x + ab.x * t, a.y + ab.y * t);
    (p - proj).length()
}

/// Paint a compact tooltip near `pointer` showing `text`. Uses the
/// wire's own color for the accent border so the user's eye links
/// the tooltip to the wire they're hovering.
fn paint_wire_tooltip(
    painter: &egui::Painter,
    pointer: egui::Pos2,
    text: &str,
    accent: egui::Color32,
) {
    let font = egui::FontId::proportional(11.0);
    let galley = painter.layout_no_wrap(
        text.to_string(),
        font,
        egui::Color32::from_rgb(235, 235, 240),
    );
    let pad = egui::vec2(6.0, 3.0);
    // Offset so the tooltip doesn't sit under the cursor.
    let min = egui::pos2(pointer.x + 12.0, pointer.y + 12.0);
    let rect = egui::Rect::from_min_size(min, galley.size() + pad * 2.0);
    painter.rect_filled(rect, 3.0, egui::Color32::from_rgba_unmultiplied(20, 22, 28, 235));
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, accent),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + pad, galley, egui::Color32::PLACEHOLDER);
}

/// Paint a small filled triangle pointing from `tail` to `tip`.
/// Used to indicate signal direction at the input end of causal
/// connections — matches `arrow={Arrow.None,Arrow.Filled}` in MLS
/// `Line` annotations.
fn paint_arrowhead(painter: &egui::Painter, tail: egui::Pos2, tip: egui::Pos2, color: egui::Color32) {
    let dx = tip.x - tail.x;
    let dy = tip.y - tail.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux); // perpendicular
    const HEAD_LEN: f32 = 9.0;
    const HEAD_HALFW: f32 = 4.0;
    let base = egui::pos2(tip.x - ux * HEAD_LEN, tip.y - uy * HEAD_LEN);
    let b1 = egui::pos2(base.x + px * HEAD_HALFW, base.y + py * HEAD_HALFW);
    let b2 = egui::pos2(base.x - px * HEAD_HALFW, base.y - py * HEAD_HALFW);
    painter.add(egui::Shape::convex_polygon(
        vec![tip, b1, b2],
        color,
        egui::Stroke::NONE,
    ));
}

/// Visual style of a port marker on a component icon. Mirrors the
/// OMEdit / Dymola convention so users can read connector causality
/// at a glance without hovering for the type name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortShape {
    /// Filled square — `input` causality (RealInput, BooleanInput, …).
    InputSquare,
    /// Filled triangle pointing outward from the icon — `output`
    /// causality (RealOutput, BooleanOutput, …).
    OutputTriangle,
    /// Filled circle — acausal physical connectors (Pin, Flange, …).
    AcausalCircle,
}

/// Paint a port marker at `center` using the OMEdit shape convention
/// described on [`PortShape`]. `dir` orients the output triangle so
/// it points away from the icon body; ignored for square / circle.
fn paint_port_shape(
    painter: &egui::Painter,
    center: egui::Pos2,
    shape: PortShape,
    dir: PortDir,
    fill: egui::Color32,
    stroke: egui::Stroke,
) {
    const R: f32 = 5.0;
    match shape {
        PortShape::InputSquare => {
            let rect = egui::Rect::from_center_size(center, egui::vec2(R * 1.6, R * 1.6));
            painter.rect_filled(rect, 0.0, fill);
            painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
        }
        PortShape::OutputTriangle => {
            // Three-point triangle: tip in `dir`, base perpendicular.
            let (ox, oy) = dir.outward();
            // For PortDir::None fall back to a small square so the
            // port is still visible (no preferred orientation).
            if (ox, oy) == (0.0, 0.0) {
                let rect = egui::Rect::from_center_size(
                    center,
                    egui::vec2(R * 1.6, R * 1.6),
                );
                painter.rect_filled(rect, 0.0, fill);
                painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
                return;
            }
            // Perpendicular for the base: rotate (ox, oy) 90°.
            let (px, py) = (-oy, ox);
            let tip = egui::pos2(center.x + ox * R * 1.4, center.y + oy * R * 1.4);
            let b1 = egui::pos2(
                center.x - ox * R * 0.4 + px * R * 0.9,
                center.y - oy * R * 0.4 + py * R * 0.9,
            );
            let b2 = egui::pos2(
                center.x - ox * R * 0.4 - px * R * 0.9,
                center.y - oy * R * 0.4 - py * R * 0.9,
            );
            let pts = vec![tip, b1, b2];
            painter.add(egui::Shape::convex_polygon(pts.clone(), fill, stroke));
        }
        PortShape::AcausalCircle => {
            painter.circle_filled(center, R - 1.0, fill);
            painter.circle_stroke(center, R - 1.0, stroke);
        }
    }
}

/// Selection-state brightener — shifts each channel ~30% toward white
/// while preserving hue. Used so wires keep their domain colour even
/// while highlighted.
fn brighten(c: egui::Color32) -> egui::Color32 {
    let lift = |v: u8| (v as u16 + 80).min(255) as u8;
    egui::Color32::from_rgb(lift(c.r()), lift(c.g()), lift(c.b()))
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
    // Generic in-canvas viz node kinds (plots today, dashboards /
    // cameras tomorrow). Lives in lunco-viz so it's reusable from any
    // domain plugin that wants embedded scopes — Modelica is just the
    // first integrator.
    lunco_viz::kinds::canvas_plot_node::register(&mut reg);
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
        let expandable_connector = data
            .get("expandable_connector")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // `icon_graphics` is the decoded Icon annotation. Missing /
        // null on MSL components — they keep using the SVG path.
        let icon_graphics = data
            .get("icon_graphics")
            .and_then(|v| {
                if v.is_null() {
                    None
                } else {
                    serde_json::from_value::<crate::annotations::Icon>(v.clone()).ok()
                }
            });
        let rotation_deg = data
            .get("icon_rotation_deg")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
            .unwrap_or(0.0);
        let mirror_x = data
            .get("icon_mirror_x")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mirror_y = data
            .get("icon_mirror_y")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let instance_name = data
            .get("instance_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        IconNodeVisual {
            type_label: type_label.clone(),
            class_name: type_label,
            icon_asset,
            icon_only,
            expandable_connector,
            icon_graphics,
            rotation_deg,
            mirror_x,
            mirror_y,
            instance_name,
        }
    });
    reg.register_edge_kind("modelica.connection", |data: &JsonValue| {
        let connector_type = data
            .get("connector_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let from_dir = PortDir::from_str(
            data.get("from_dir").and_then(|v| v.as_str()).unwrap_or(""),
        );
        let to_dir = PortDir::from_str(
            data.get("to_dir").and_then(|v| v.as_str()).unwrap_or(""),
        );
        // Waypoints come in as Modelica coords (+Y up). Flip Y here
        // so the renderer can walk them in the same world frame as
        // the port positions (nodes already live in the flipped
        // space, see the IconTransform comment).
        let waypoints_world: Vec<CanvasPos> = data
            .get("waypoints")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|pt| {
                        let pt = pt.as_array()?;
                        let x = pt.first()?.as_f64()? as f32;
                        let y = pt.get(1)?.as_f64()? as f32;
                        Some(CanvasPos::new(x, -y))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Prefer the connector's Icon-derived color (OMEdit/Dymola
        // convention) when the projector populated it, fall through
        // to the leaf-name palette otherwise.
        let icon_color = data
            .get("icon_color")
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                let r = arr.first()?.as_u64()? as u8;
                let g = arr.get(1)?.as_u64()? as u8;
                let b = arr.get(2)?.as_u64()? as u8;
                Some(egui::Color32::from_rgb(r, g, b))
            });
        let leaf = connector_type.rsplit('.').next().unwrap_or(connector_type);
        let source_path = data
            .get("source_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let target_path = data
            .get("target_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // AST-derived connector classification (Input/Output/Acausal)
        // replaces the old "ends_with Input" leaf-name test.
        let kind = match data.get("kind").and_then(|v| v.as_str()).unwrap_or("") {
            "input" => crate::visual_diagram::PortKind::Input,
            "output" => crate::visual_diagram::PortKind::Output,
            _ => crate::visual_diagram::PortKind::Acausal,
        };
        let is_causal = matches!(
            kind,
            crate::visual_diagram::PortKind::Input | crate::visual_diagram::PortKind::Output,
        );

        let flow_vars: Vec<crate::visual_diagram::FlowVarMeta> = data
            .get("flow_vars")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        OrthogonalEdgeVisual {
            color: icon_color.unwrap_or_else(|| wire_color_for(connector_type)),
            from_dir,
            to_dir,
            waypoints_world,
            is_causal,
            source_path,
            target_path,
            kind,
            flow_vars,
            connector_leaf: leaf.to_string(),
        }
    });
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
/// walks up both sides for any additional ports. Uses default icon
/// dimensions; for sized icons see [`port_fallback_offset_for_size`].
fn port_fallback_offset(index: usize, total: usize) -> (f32, f32) {
    port_fallback_offset_for_size(index, total, ICON_W, ICON_H)
}

/// Same fallback layout as [`port_fallback_offset`] but parameterised
/// by the icon's actual width/height — needed once Placement-driven
/// node sizing makes per-instance dimensions vary instead of always
/// being 20×20.
fn port_fallback_offset_for_size(
    index: usize,
    _total: usize,
    icon_w: f32,
    icon_h: f32,
) -> (f32, f32) {
    let side_left = index % 2 == 0;
    let row = index / 2; // 0 → middle, 1 → above, 2 → even higher
    let cy = icon_h * 0.5 - (row as f32) * (icon_h * 0.25);
    let cx = if side_left { 0.0 } else { icon_w };
    (cx, cy.clamp(0.0, icon_h))
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
        // The single source of truth for this node's icon-local →
        // canvas-world transform. Built once by the importer from the
        // Placement, applied uniformly here for the rect, ports, and
        // (eventually) the icon body.
        let xform = node.icon_transform;

        // Bounding rect = AABB of the icon's local extent
        // ({{-100,-100},{100,100}} per MLS default) under the
        // transform. Honours rotation naturally (a 45°-rotated icon
        // gets a larger axis-aligned rect than its unrotated form).
        let ((min_wx, min_wy), (max_wx, max_wy)) =
            xform.local_aabb(-100.0, -100.0, 100.0, 100.0);
        let icon_w_local = (max_wx - min_wx).max(4.0);
        let icon_h_local = (max_wy - min_wy).max(4.0);

        let n_ports = node.component_def.ports.len();
        let ports: Vec<CanvasPort> = node
            .component_def
            .ports
            .iter()
            .enumerate()
            .map(|(i, p)| {
                // Port positions in icon-local Modelica coords go
                // through the same transform — no per-feature
                // mirror/rotate branches, just one matrix multiply.
                // The result is in canvas world; we convert to
                // icon-local *screen* coords (relative to the rect's
                // top-left) since `CanvasPort.local_offset` is icon-
                // local, not world.
                let (wx, wy) = if p.x == 0.0 && p.y == 0.0 {
                    // Fallback layout: distribute around the rect.
                    // Already in icon-local screen coords — convert
                    // to world by adding the rect's top-left.
                    let (fx, fy) = port_fallback_offset_for_size(
                        i,
                        n_ports,
                        icon_w_local,
                        icon_h_local,
                    );
                    (min_wx + fx, min_wy + fy)
                } else {
                    xform.apply(p.x, p.y)
                };
                let lx = wx - min_wx;
                let ly = wy - min_wy;
                CanvasPort {
                    id: CanvasPortId::new(p.name.clone()),
                    local_offset: CanvasPos::new(lx, ly),
                    // AST-derived causality classification as a short
                    // string (`"input"` / `"output"` / `"acausal"`) —
                    // the canvas renderer's port-shape match reads
                    // this directly, so MSL naming conventions are
                    // no longer needed to pick the right shape.
                    kind: port_kind_str(p.kind).into(),
                }
            })
            .collect();

        scene.insert_node(CanvasNode {
            id: cid,
            rect: CanvasRect::from_min_size(
                CanvasPos::new(min_wx, min_wy),
                icon_w_local,
                icon_h_local,
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
                // `expandable connector` (MLS §9.1.3) — rendered with
                // a dashed accent border. Set by the projector from
                // the class-def's `expandable` flag via
                // `register_local_class`; MSL palette entries carry
                // the flag through the MSLComponentDef.
                "expandable_connector": node.component_def.is_expandable_connector,
                // Decoded `Icon(graphics={...})` annotation for the
                // class, if the projector extracted one. Takes
                // precedence over the SVG fallback in
                // `IconNodeVisual::draw`. Omitted when `None` so the
                // common (MSL) path produces the same JSON it always
                // has.
                "icon_graphics": node.component_def.icon_graphics,
                // Orientation parameters (rotation + mirror) preserved
                // alongside the node's `IconTransform` matrix. The
                // visual reads these to rotate/mirror the icon body
                // itself, complementing the port positions handled
                // above. Only the named primitives travel — the matrix
                // is rebuilt from `extent` + `position` when needed
                // (translation/scale are already baked into the
                // canvas rect).
                "icon_rotation_deg": node.icon_transform.rotation_deg,
                "icon_mirror_x": node.icon_transform.mirror_x,
                "icon_mirror_y": node.icon_transform.mirror_y,
                // Carried through so the icon renderer can substitute
                // `%name` in authored `Text(textString="%name")`
                // primitives — the reason every MSL component shows
                // "R1" / "C1" on its body instead of the class name.
                "instance_name": node.instance_name,
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

        // Look up the source / target port definitions so we can
        // bake connector type + edge-side direction into the edge's
        // data. The visual reads both for colour selection and
        // port-direction stubs without needing world access.
        let src_node = diagram.nodes.iter().find(|n| n.id == edge.source_node);
        let tgt_node = diagram.nodes.iter().find(|n| n.id == edge.target_node);
        // Port lookup falls back to the head segment so qualified
        // sub-port references like `flange.phi` (from
        // `recover_edges_from_source`) still resolve to the
        // outer `flange` PortDef. Without this, every recovered
        // edge with a sub-port lost its colour + stub direction
        // because the find() returned None.
        let find_port = |defs: &[crate::visual_diagram::PortDef], name: &str|
            -> Option<crate::visual_diagram::PortDef>
        {
            if let Some(p) = defs.iter().find(|p| p.name == name) {
                return Some(p.clone());
            }
            let head = name.split('.').next().unwrap_or(name);
            defs.iter().find(|p| p.name == head).cloned()
        };
        let src_port_def =
            src_node.and_then(|n| find_port(&n.component_def.ports, &edge.source_port));
        let tgt_port_def =
            tgt_node.and_then(|n| find_port(&n.component_def.ports, &edge.target_port));
        let connector_type = src_port_def
            .as_ref()
            .map(|p| p.connector_type.clone())
            .unwrap_or_default();
        // Wire color sourced from the connector class's Icon
        // (populated by the projector for both local & MSL types).
        // Falls back to `null` so the edge factory uses the leaf-name
        // palette in `wire_color_for`.
        let icon_color = src_port_def
            .as_ref()
            .and_then(|p| p.color)
            .or_else(|| tgt_port_def.as_ref().and_then(|p| p.color));
        // Stub direction = which edge the port sits on in *screen*
        // space. Apply the owning instance's transform's linear part
        // (no translation — directions don't have a position). One
        // matrix multiply per port replaces the previous four
        // per-feature branches (mirror_x, mirror_y, rotate_x, …).
        let from_dir = match (src_node, src_port_def.as_ref()) {
            (Some(n), Some(p)) => {
                let (dx, dy) = n.icon_transform.apply_dir(p.x, p.y);
                port_edge_dir(dx, dy)
            }
            _ => PortDir::None,
        };
        let to_dir = match (tgt_node, tgt_port_def.as_ref()) {
            (Some(n), Some(p)) => {
                let (dx, dy) = n.icon_transform.apply_dir(p.x, p.y);
                port_edge_dir(dx, dy)
            }
            _ => PortDir::None,
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
            data: serde_json::json!({
                "connector_type": connector_type,
                "from_dir": from_dir.as_str(),
                "to_dir": to_dir.as_str(),
                // Authored polyline in Modelica coords (+Y up); Y is
                // flipped to canvas world coords at render time.
                // Empty array when the edge uses auto-routing only.
                "waypoints": edge.waypoints,
                // Icon-derived color [r,g,b] when available; null
                // means the edge factory should use wire_color_for(type).
                "icon_color": icon_color,
                // Fully-qualified port paths for hover tooltips —
                // e.g. `"engine.thrust"` or `"tank.fuel_out"`. The
                // renderer appends each flow variable's name for
                // acausal ports and looks the resulting path up in
                // the per-frame NodeStateSnapshot.
                "source_path": src_node
                    .map(|n| format!("{}.{}", n.instance_name, edge.source_port))
                    .unwrap_or_default(),
                "target_path": tgt_node
                    .map(|n| format!("{}.{}", n.instance_name, edge.target_port))
                    .unwrap_or_default(),
                // AST-derived connector semantics — replace the old
                // leaf-name heuristics. `kind` is Input/Output/Acausal
                // (drives arrowhead + animation eligibility).
                // `flow_vars` lists every `flow` variable on the
                // connector class (name + declared unit) so the
                // tooltip / animation can reference them by their
                // real names instead of a hardcoded `m_dot`.
                "kind": src_port_def
                    .as_ref()
                    .map(|p| port_kind_str(p.kind))
                    .unwrap_or("acausal"),
                "flow_vars": src_port_def
                    .as_ref()
                    .map(|p| p.flow_vars.clone())
                    .unwrap_or_default(),
            }),
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
/// Shared handle to the target class's `Diagram(graphics={...})`
/// annotation — painted as canvas background by
/// [`DiagramDecorationLayer`]. Projector updates it each time the
/// drilled-in class changes.
pub type BackgroundDiagramHandle = std::sync::Arc<
    std::sync::RwLock<
        Option<(
            crate::annotations::CoordinateSystem,
            Vec<crate::annotations::GraphicItem>,
        )>,
    >,
>;

#[allow(dead_code)]
#[cfg(any())]
fn render_canvas_plots_deprecated(
    ui: &mut bevy_egui::egui::Ui,
    world: &mut World,
    active_doc: Option<lunco_doc::DocumentId>,
    canvas_screen_rect: bevy_egui::egui::Rect,
) {
    use bevy_egui::egui;
    use egui_plot::{Line, Plot, PlotPoints};
    let Some(active_doc) = active_doc else { return };

    // Snapshot plot list + viewport so we don't hold the docstate
    // borrow across egui_plot calls.
    let (plots, viewport) = {
        let state = world.resource::<CanvasDiagramState>();
        let docstate = state.get(Some(active_doc));
        if docstate.canvas_plots.is_empty() {
            return;
        }
        (
            docstate.canvas_plots.clone(),
            docstate.canvas.viewport.clone(),
        )
    };

    // Look up the active simulator entity once — same lookup
    // NewPlotPanel uses to bind signal refs.
    let model_entity = world
        .query::<(bevy::prelude::Entity, &crate::ModelicaModel)>()
        .iter(world)
        .next()
        .map(|(e, _)| e)
        .unwrap_or(bevy::prelude::Entity::PLACEHOLDER);

    let canvas_rect = lunco_canvas::Rect::from_min_max(
        lunco_canvas::Pos::new(canvas_screen_rect.min.x, canvas_screen_rect.min.y),
        lunco_canvas::Pos::new(canvas_screen_rect.max.x, canvas_screen_rect.max.y),
    );

    // Pull SignalRegistry once — it's a Resource we read for every
    // plot below, no mutation.
    let registry_present =
        world.get_resource::<lunco_viz::SignalRegistry>().is_some();
    if !registry_present {
        return;
    }

    for (idx, plot) in plots.iter().enumerate() {
        let screen_rect =
            viewport.world_rect_to_screen(
                lunco_canvas::Rect::from_min_max(plot.world_min, plot.world_max),
                canvas_rect,
            );
        let egui_rect = egui::Rect::from_min_max(
            egui::pos2(screen_rect.min.x, screen_rect.min.y),
            egui::pos2(screen_rect.max.x, screen_rect.max.y),
        );
        // Skip plots fully outside the visible canvas area —
        // pan/zoom can move them off-screen and rendering an
        // off-canvas widget wastes layout time.
        if !canvas_screen_rect.intersects(egui_rect) {
            continue;
        }

        // Build the line points from SignalRegistry. Re-acquire
        // the resource borrow per-plot so future per-plot
        // multi-signal lookups stay simple.
        let signal_ref =
            lunco_viz::SignalRef::new(model_entity, plot.signal_path.clone());
        let points: Vec<[f64; 2]> = world
            .resource::<lunco_viz::SignalRegistry>()
            .scalar_history(&signal_ref)
            .map(|h| h.samples.iter().map(|s| [s.time, s.value]).collect())
            .unwrap_or_default();

        // Foreground layer so the plot draws on top of nodes/wires.
        let fg_layer = egui::LayerId::new(
            egui::Order::Foreground,
            ui.id().with(("canvas_plot", active_doc.raw(), idx)),
        );
        let painter = ui.ctx().layer_painter(fg_layer);
        // Card background so the plot stays readable over busy
        // diagrams. Theme-driven colours come from the canvas
        // overlay theme already used by the NavBar overlay.
        let theme = lunco_canvas::theme::current(ui.ctx());
        painter.rect_filled(egui_rect, 6.0, theme.overlay_fill);
        painter.rect_stroke(
            egui_rect,
            6.0,
            egui::Stroke::new(1.0, theme.overlay_stroke),
            egui::StrokeKind::Outside,
        );

        // Plot body — small egui_plot inside the rect. Title bar
        // shows the bound signal name.
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui_rect.shrink(4.0))
                .layout(egui::Layout::top_down(egui::Align::Min))
                .layer_id(fg_layer),
        );
        child.label(
            egui::RichText::new(&plot.signal_path)
                .small()
                .color(theme.overlay_text),
        );
        let plot_id = (
            "lunco_canvas_plot",
            active_doc.raw(),
            idx as u64,
        );
        Plot::new(plot_id)
            .show_axes([false, false])
            .show_grid(false)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .show(&mut child, |plot_ui| {
                if !points.is_empty() {
                    plot_ui.line(Line::new("", PlotPoints::from(points)));
                }
            });
    }
}

pub struct CanvasDocState {
    pub canvas: Canvas,
    pub last_seen_gen: u64,
    /// Hash of the *projection-relevant* slice of source for the
    /// scene currently on screen — collapses whitespace, drops
    /// comments. Cheap-skip: when a doc generation bumps but this
    /// hash is unchanged (a comment edit, a parameter-default tweak,
    /// added blank lines), we mark the gen as seen without spawning
    /// a projection task. Catches the bulk of typing latency.
    ///
    /// TODO(partial-reproject): replace this binary skip with an
    /// AST-diff path. Compare prev vs new `ClassDef.components` /
    /// `equations` / annotations, emit a sequence of
    /// `DiagramOp { AddNode | RemoveNode | MoveNode | AddEdge |
    /// RemoveEdge | RelabelNode }`, and apply each to `canvas.scene`
    /// in place. Falls back to full reproject on extends/within/
    /// multi-class changes. Needs (1) Scene mutation API surface
    /// (move/relabel/add-without-rebuild), (2) `diff_class(old,
    /// new) -> Vec<DiagramOp>` helper, (3) origin-name as stable
    /// node identity (already true). 30 % of edits hit the partial
    /// path — see <follow-up issue> when ready.
    pub last_seen_source_hash: u64,
    /// MSL pre-warm generation observed at the last successful
    /// projection. When `class_cache::msl_prewarm_generation()`
    /// advances past this, the projection is forced to re-run so
    /// inherited components surfaced by the warm cache appear.
    pub last_seen_prewarm_gen: u64,
    /// Set by the [`crate::ui::commands::FitCanvas`] observer; the
    /// canvas render system consumes it next frame and runs Fit
    /// against the *actual* widget rect (rather than the hardcoded
    /// 800×600 the observer would have to use). Cleared after the
    /// fit lands.
    pub pending_fit: bool,
    /// Snapshot of the drill-in target that produced the *currently
    /// rendered* scene. The render trigger compares this against the
    /// live `DrilledInClassNames[doc_id]`; a difference re-projects.
    /// Without this, clicking a class in the Twin Browser updated the
    /// drill-in resource but the canvas kept showing the previous
    /// target's cached scene — the visible "click did nothing" bug.
    pub last_seen_target: Option<String>,
    pub context_menu: Option<PendingContextMenu>,
    pub projection_task: Option<ProjectionTask>,
    /// Background decoration — the target class's own
    /// `Diagram(graphics={...})` annotation. Painted by the
    /// decoration layer registered on `canvas`. Shared via `Arc` so
    /// the projection code can update the layer's data without
    /// reaching into `canvas.layers`.
    pub background_diagram: BackgroundDiagramHandle,
}

impl Default for CanvasDocState {
    fn default() -> Self {
        let mut canvas = Canvas::new(build_registry());
        canvas.layers.retain(|layer| layer.name() != "selection");
        canvas.overlays.push(Box::new(NavBarOverlay::default()));
        // Diagram decoration layer sits right after the grid so it
        // paints behind nodes and edges. The decoration data is
        // shared via `Arc<RwLock>` with `CanvasDocState` so the
        // projector can swap in a new class's graphics without
        // walking the layer list.
        let background_diagram: BackgroundDiagramHandle =
            std::sync::Arc::new(std::sync::RwLock::new(None));
        let decoration_idx = canvas
            .layers
            .iter()
            .position(|l| l.name() != "grid")
            .unwrap_or(canvas.layers.len());
        canvas.layers.insert(
            decoration_idx,
            Box::new(DiagramDecorationLayer {
                data: background_diagram.clone(),
            }),
        );
        Self {
            canvas,
            last_seen_gen: 0,
            last_seen_source_hash: 0,
            last_seen_prewarm_gen: 0,
            pending_fit: false,
            last_seen_target: None,
            context_menu: None,
            projection_task: None,
            background_diagram,
        }
    }
}

/// Hash the *projection-relevant* slice of source — collapses runs
/// of whitespace into single spaces and drops `//` line comments
/// and `/* … */` block comments. String literals are preserved
/// (they include filenames in `Bitmap(fileName=...)` annotations,
/// which DO affect rendering).
///
/// Used by the cheap "edit-class skip": when the document
/// generation bumps but this hash hasn't moved, the edit was a
/// comment / blank-line / parameter-default tweak that doesn't
/// change the projected scene topology — skip the projection task
/// entirely. Catches the bulk of the typing-latency regressions on
/// large MSL files.
///
/// Note: false negatives (edits that DO change projection but
/// produce the same hash) are impossible — the hash domain
/// includes every glyph in components / equations / annotations.
/// False positives (edits that DON'T change projection but bump
/// the hash) are fine: we just over-project, same as before.
fn projection_relevant_source_hash(source: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    let mut last_was_ws = true;
    while let Some(c) = chars.next() {
        if in_string {
            c.hash(&mut h);
            if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '/' {
            match chars.peek() {
                Some('/') => {
                    chars.next();
                    while let Some(&n) = chars.peek() {
                        if n == '\n' { break; }
                        chars.next();
                    }
                    continue;
                }
                Some('*') => {
                    chars.next();
                    while let Some(c2) = chars.next() {
                        if c2 == '*' && chars.peek() == Some(&'/') {
                            chars.next();
                            break;
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }
        if c == '"' {
            in_string = true;
            c.hash(&mut h);
            last_was_ws = false;
            continue;
        }
        if c.is_whitespace() {
            if !last_was_ws {
                ' '.hash(&mut h);
                last_was_ws = true;
            }
            continue;
        }
        c.hash(&mut h);
        last_was_ws = false;
    }
    h.finish()
}

/// Paints the target class's `Diagram(graphics={...})` annotation as
/// canvas background — the red labelled rectangles, text callouts,
/// and accent lines MSL example diagrams carry for reader orientation
/// (the PID example's "reference speed generation" / "PI controller"
/// / "plant" regions are the canonical case). Holds an
/// `Arc<RwLock<…>>` handle so the projector can push a new class's
/// graphics in without reaching into the canvas's layer list.
struct DiagramDecorationLayer {
    data: BackgroundDiagramHandle,
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
        let Some((coord_system, graphics)) = guard.as_ref() else {
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
        let screen_rect_canvas =
            ctx.viewport.world_rect_to_screen(world_rect, ctx.screen_rect);
        let screen_rect = bevy_egui::egui::Rect::from_min_max(
            bevy_egui::egui::pos2(screen_rect_canvas.min.x, screen_rect_canvas.min.y),
            bevy_egui::egui::pos2(screen_rect_canvas.max.x, screen_rect_canvas.max.y),
        );
        crate::icon_paint::paint_graphics(
            ctx.ui.painter(),
            screen_rect,
            *coord_system,
            graphics,
        );
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
    /// Drill-in target the projection was spawned for. Compared
    /// against `CanvasDocState::last_seen_target` on completion so
    /// the UI knows which target produced the rendered scene.
    pub target_at_spawn: Option<String>,
    pub spawned_at: std::time::Instant,
    pub deadline: std::time::Duration,
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub task: bevy::tasks::Task<Scene>,
    /// Projection-relevant source hash captured at spawn time.
    /// Stashed onto `CanvasDocState::last_seen_source_hash` when the
    /// task completes — used by the next gen-bump check to skip
    /// reprojection on no-op edits (whitespace, comments).
    pub source_hash: u64,
    /// MSL pre-warm generation observed at spawn time. Saved onto
    /// `CanvasDocState::last_seen_prewarm_gen` when the task
    /// completes — *not* the live value at completion time, so a
    /// pre-warm that landed mid-projection still triggers a
    /// follow-up reproject.
    pub prewarm_gen_at_spawn: u64,
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
            // Active doc from the Workspace session (source of truth);
            // `WorkbenchState.open_model` is still read below for
            // display-cache fields, but no longer for identity.
            let Some(doc_id) = world
                .resource::<lunco_workbench::WorkspaceResource>()
                .active_document
            else {
                world
                    .resource_mut::<CanvasDiagramState>()
                    .get_mut(None)
                    .canvas
                    .scene = Scene::new();
                self.render_canvas(ui, world);
                return;
            };
            if world.resource::<WorkbenchState>().open_model.is_none() {
                self.render_canvas(ui, world);
                return;
            }
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
            // Drill-in target changed (e.g. user clicked a different
            // class in the Twin Browser for an already-open tab).
            let live_target = world
                .get_resource::<DrilledInClassNames>()
                .and_then(|m| m.get(doc_id).map(str::to_string));
            let target_changed = live_target != docstate.last_seen_target;
            let prewarm_advanced = crate::class_cache::msl_prewarm_generation()
                != docstate.last_seen_prewarm_gen;
            // Hash-skip: when the gen bumped but the projection-
            // relevant source slice (whitespace-collapsed, comment-
            // stripped) is unchanged, mark the gen as seen and bail
            // out without spawning a projection task. This is the
            // cheap layer of "intelligent reprojection" — catches
            // comment / blank-line / parameter-default edits before
            // they pay the rumoca-projection cost.
            let needs_project = first_render || target_changed || prewarm_advanced || {
                if !gen_advanced {
                    false
                } else {
                    let new_hash = world
                        .resource::<ModelicaDocumentRegistry>()
                        .host(doc_id)
                        .map(|h| projection_relevant_source_hash(h.document().source()))
                        .unwrap_or(0);
                    if new_hash == docstate.last_seen_source_hash {
                        // Mark the gen as seen so the render loop
                        // doesn't keep re-checking every frame.
                        // Drop the read-only borrow first.
                        drop(state);
                        let mut state =
                            world.resource_mut::<CanvasDiagramState>();
                        let docstate = state.get_mut(Some(doc_id));
                        docstate.last_seen_gen = gen;
                        bevy::log::debug!(
                            "[CanvasDiagram] skipping reproject for gen={gen} (source-hash unchanged)"
                        );
                        false
                    } else {
                        true
                    }
                }
            };
            needs_project.then_some((doc_id, gen))
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
            // Snapshot the auto-layout grid so the bg task can fall
            // back to configurable spacing for components without a
            // `Placement` annotation.
            let layout_snapshot = world
                .get_resource::<crate::ui::panels::diagram::DiagramAutoLayoutSettings>()
                .cloned()
                .unwrap_or_default();
            let mut state = world.resource_mut::<CanvasDiagramState>();
            let docstate = state.get_mut(Some(doc_id));
            // If the user just changed drill-in target (clicked a
            // different class in the Twin Browser), the new scene's
            // bounds usually have nothing to do with the old one —
            // so we want auto-fit to engage exactly as it does on a
            // fresh tab open. Resetting `last_seen_gen` to 0 makes
            // the completion path treat this projection as
            // "initial" and refit the viewport, instead of leaving
            // the camera at the stale zoom that made the new icons
            // look "way too far apart" (icons rendered at near 1:1
            // because the previous package-level scene auto-fit
            // happened at a different scale).
            if docstate.last_seen_target != target_class_snapshot {
                docstate.last_seen_gen = 0;
            }
            // Refresh the Diagram-annotation background decoration
            // for the target class. Cheap AST walk (no re-parse —
            // `ast_arc` is the already-parsed tree the task below
            // consumes). Runs on main thread; `paint_graphics` is
            // idle until the layer's next draw.
            let bg_handle = docstate.background_diagram.clone();
            if let Some(ast) = ast_arc.as_ref() {
                let diag = diagram_annotation_for_target(
                    ast.as_ref(),
                    target_class_snapshot.as_deref(),
                );
                if let Ok(mut guard) = bg_handle.write() {
                    *guard = diag.map(|d| (d.coordinate_system, d.graphics));
                }
            } else if let Ok(mut guard) = bg_handle.write() {
                *guard = None;
            }
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
                    // Compute the hash now (off the move into the task)
                    // so the completion handler can stash it on the
                    // docstate without re-fetching the source.
                    let source_hash_at_spawn =
                        projection_relevant_source_hash(&source);
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
                                &layout_snapshot,
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
                        target_at_spawn: target_class_snapshot.clone(),
                        spawned_at,
                        deadline,
                        cancel,
                        task,
                        source_hash: source_hash_at_spawn,
                        prewarm_gen_at_spawn: crate::class_cache::msl_prewarm_generation(),
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
                .resource::<lunco_workbench::WorkspaceResource>()
                .active_document;
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
                    .map(|scene| {
                        (
                            t.gen_at_spawn,
                            t.target_at_spawn.clone(),
                            t.source_hash,
                            t.prewarm_gen_at_spawn,
                            scene,
                        )
                    })
                });
            if let Some((gen, target, source_hash, prewarm_gen_at_spawn, scene)) = done_task {
                docstate.projection_task = None;
                bevy::log::info!(
                    "[CanvasDiagram] project done: {} nodes, {} edges (initial={})",
                    scene.node_count(),
                    scene.edge_count(),
                    is_initial_projection,
                );
                // Preserve the user's selection across re-projection
                // when the same node is still in the new scene — the
                // prior unconditional `clear()` made every drag /
                // small edit feel like a visual reset (the highlight
                // ring would briefly vanish after each SetPlacement
                // cycle). We match nodes by `origin` (= the Modelica
                // instance name) since Bevy IDs change across scene
                // rebuilds.
                let preserved_origins: std::collections::HashSet<String> = docstate
                    .canvas
                    .selection
                    .iter()
                    .filter_map(|sid| match sid {
                        lunco_canvas::SelectItem::Node(nid) => docstate
                            .canvas
                            .scene
                            .node(*nid)
                            .and_then(|n| n.origin.clone()),
                        _ => None,
                    })
                    .collect();
                // Build an old-id → new-id map keyed by node origin
                // (Modelica instance name) so we can remap any in-
                // flight tool gesture (press / drag / connect)
                // across the wholesale scene swap. The pre-existing
                // symptom users saw was "first click+drag does
                // nothing, second one works": the press registered
                // against a NodeId from the old scene, the move
                // handler couldn't promote to a drag because
                // `scene.node(id)` returned None for the stale id.
                // Remapping by origin preserves the gesture across
                // re-projection so the first attempt completes.
                let old_origin_to_id: std::collections::HashMap<String, lunco_canvas::NodeId> =
                    docstate
                        .canvas
                        .scene
                        .nodes()
                        .filter_map(|(id, n)| n.origin.clone().map(|o| (o, *id)))
                        .collect();
                let new_origin_to_id: std::collections::HashMap<String, lunco_canvas::NodeId> =
                    scene
                        .nodes()
                        .filter_map(|(id, n)| n.origin.clone().map(|o| (o, *id)))
                        .collect();
                let id_remap: std::collections::HashMap<lunco_canvas::NodeId, lunco_canvas::NodeId> =
                    old_origin_to_id
                        .iter()
                        .filter_map(|(origin, old_id)| {
                            new_origin_to_id.get(origin).map(|new_id| (*old_id, *new_id))
                        })
                        .collect();
                docstate.canvas.tool.remap_node_ids(&|old: lunco_canvas::NodeId| {
                    id_remap.get(&old).copied()
                });
                docstate.canvas.scene = scene;
                docstate.canvas.selection.clear();
                if !preserved_origins.is_empty() {
                    let new_ids: Vec<lunco_canvas::NodeId> = docstate
                        .canvas
                        .scene
                        .nodes()
                        .filter_map(|(nid, n)| {
                            n.origin
                                .as_deref()
                                .filter(|o| preserved_origins.contains(*o))
                                .map(|_| *nid)
                        })
                        .collect();
                    for id in new_ids {
                        docstate.canvas.selection.add(lunco_canvas::SelectItem::Node(id));
                    }
                }
                docstate.last_seen_gen = gen;
                docstate.last_seen_target = target;
                // Cache the projection-relevant source hash that the
                // task captured at spawn time. Next frame's
                // gen-advanced check skips reprojection when the
                // current source hashes to the same value (comment /
                // whitespace edit). Best-effort: if a newer edit
                // landed mid-projection, the hash will differ from
                // current source — gen-advanced check will then
                // trigger the follow-up projection, correct.
                docstate.last_seen_source_hash = source_hash;
                // Use the value captured at SPAWN — if a pre-warm
                // landed mid-projection, the live counter has
                // advanced past this and the next frame's
                // `prewarm_advanced` check will trigger the
                // follow-up reproject that picks up the new
                // inherited components.
                docstate.last_seen_prewarm_gen = prewarm_gen_at_spawn;
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

        // Render the canvas and collect its events. Flip the
        // canvas's `read_only` flag so the tool layer refuses to
        // enter drag/connect/delete states — pan + zoom + selection
        // still work. Authored scene mutations are blocked at the
        // input source, not corrected after the fact.
        // Snap settings come from a long-lived resource that the
        // Settings menu toggles. Read it here each frame and push
        // onto the canvas so the tool sees an up-to-date value
        // during the next drag update. Off by default — users turn
        // it on when they want drag alignment.
        let snap_settings: Option<lunco_canvas::SnapSettings> = world
            .get_resource::<CanvasSnapSettings>()
            .filter(|s| s.enabled)
            .map(|s| lunco_canvas::SnapSettings { step: s.step });

        // Theme snapshot: computed once per render and stashed in the
        // egui context so the NodeVisual / EdgeVisual trait objects
        // inside `canvas.ui` (which have no `World` access) can still
        // pick theme-aware colours on draw.
        {
            let theme = world
                .get_resource::<lunco_theme::Theme>()
                .cloned()
                .unwrap_or_else(lunco_theme::Theme::dark);
            store_canvas_theme(
                ui.ctx(),
                CanvasThemeSnapshot::from_theme(&theme),
            );
            lunco_canvas::theme::store(
                ui.ctx(),
                layer_theme_from(&theme),
            );
        }

        // Stash a per-frame snapshot of `SignalRegistry` data so any
        // `lunco.viz.plot` scene nodes drawn this frame can read live
        // samples without a `World` reference. Visuals live in
        // `lunco-viz` and have no Bevy access; the snapshot is the
        // bridge. Empty when no SignalRegistry is installed —
        // `PlotNodeVisual` degrades to "title only".
        if let Some(sig_reg) = world.get_resource::<lunco_viz::SignalRegistry>() {
            let mut snapshot =
                lunco_viz::kinds::canvas_plot_node::SignalSnapshot::default();
            for (sig_ref, hist) in sig_reg.iter_scalar() {
                let pts: Vec<[f64; 2]> =
                    hist.samples.iter().map(|s| [s.time, s.value]).collect();
                snapshot
                    .samples
                    .insert((sig_ref.entity, sig_ref.path.clone()), pts);
            }
            lunco_viz::kinds::canvas_plot_node::stash_signal_snapshot(
                ui.ctx(),
                snapshot,
            );
        }

        // Stash a flat per-instance value snapshot so node visuals
        // (icon hover tooltips, future inline value badges, etc.)
        // can read parameters / inputs / live variables without
        // touching the World. Combines all three buckets keyed by
        // dotted instance path (`R1.R`, `P.y`, …); visuals filter by
        // the prefix that matches their instance name.
        {
            let mut state =
                lunco_viz::kinds::canvas_plot_node::NodeStateSnapshot::default();
            let mut q = world.query::<&crate::ModelicaModel>();
            for model in q.iter(world) {
                for (k, v) in &model.parameters {
                    state.values.insert(k.clone(), *v);
                }
                for (k, v) in &model.inputs {
                    state.values.insert(k.clone(), *v);
                }
                for (k, v) in &model.variables {
                    state.values.insert(k.clone(), *v);
                }
            }
            lunco_viz::kinds::canvas_plot_node::stash_node_state(
                ui.ctx(),
                state,
            );
            // Stash "simulation is actively stepping" so edge visuals
            // know when to animate. Animation is a *status* indicator:
            // no step → no dots, regardless of last-sampled flow
            // values. Read off `ModelicaModel.paused` across all live
            // models on the scene; any unpaused model counts.
            let any_unpaused = {
                let mut q2 = world.query::<&crate::ModelicaModel>();
                q2.iter(world).any(|m| !m.paused)
            };
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    egui::Id::new("lunco_modelica_sim_stepping"),
                    any_unpaused,
                );
            });
        }

        let (response, events) = {
            let mut state = world.resource_mut::<CanvasDiagramState>();
            let docstate = state.get_mut(active_doc);
            docstate.canvas.read_only = tab_read_only;
            docstate.canvas.snap = snap_settings;
            docstate.canvas.ui(ui)
        };

        // Service a deferred Fit request now that the widget rect
        // (`response.rect`) is known. The observer side just sets
        // the flag so the math runs against the real screen size.
        {
            let mut state = world.resource_mut::<CanvasDiagramState>();
            let docstate = state.get_mut(active_doc);
            if docstate.pending_fit {
                docstate.pending_fit = false;
                if let Some(bounds) = docstate.canvas.scene.bounds() {
                    let sr = lunco_canvas::Rect::from_min_max(
                        lunco_canvas::Pos::new(response.rect.min.x, response.rect.min.y),
                        lunco_canvas::Pos::new(response.rect.max.x, response.rect.max.y),
                    );
                    let (c, z) = docstate.canvas.viewport.fit_values(bounds, sr, 40.0);
                    docstate.canvas.viewport.set_target(c, z);
                }
            }
        }

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
            let dup_loads = world.resource::<DuplicateLoads>();
            let docstate = state.get(active_doc);
            // Unify drill-in + duplicate into a single loading
            // overlay — both are "document is being built off-thread,
            // canvas will populate when the bg task lands."
            let info = active_doc.and_then(|d| {
                loads
                    .progress(d)
                    .or_else(|| dup_loads.progress(d))
                    .map(|(q, secs)| (q.to_string(), secs))
            });
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
        let theme_snapshot_for_overlay = world
            .get_resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        if let Some((class, secs)) = loading_info {
            if !scene_has_content {
                render_drill_in_loading_overlay(ui, response.rect, &class, secs, &theme_snapshot_for_overlay);
            }
        } else if projecting && !scene_has_content {
            render_projecting_overlay(ui, response.rect, &theme_snapshot_for_overlay);
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
    // ── Add Plot ──────────────────────────────────────────────────
    // In-canvas scope: pick a signal from the active simulator and
    // drop a `lunco.viz.plot` Scene node at the click position.
    // Empty submenu means no sim has run yet.
    let sigs: Vec<(bevy::prelude::Entity, String)> = world
        .get_resource::<lunco_viz::SignalRegistry>()
        .map(|r| {
            let mut v: Vec<_> = r
                .iter_scalar()
                .map(|(s, _)| (s.entity, s.path.clone()))
                .collect();
            v.sort_by(|a, b| a.1.cmp(&b.1));
            v
        })
        .unwrap_or_default();
    ui.menu_button("📊 Add Plot here", |ui| {
        // TODO(menu-height): the height is "so-so" — sometimes
        // collapses to 3 rows. Match how the Modelica
        // "Add component" cascade works (see
        // `render_msl_package_menu` ~3065): plain
        // `ui.menu_button(..., |ui| ...)` recursively, no explicit
        // `set_min_*`/`set_max_*`. Egui auto-sizes from content
        // there and it Just Works. The current adaptive
        // computation below is a workaround — the real fix is to
        // mirror that simpler structure (probably means dropping
        // the ScrollArea wrapper too).
        const ROW_PX: f32 = 18.0;
        let max_h = (ui.ctx().screen_rect().height() * 0.7).max(180.0);
        let wanted = ((sigs.len() + 2) as f32 * ROW_PX).min(max_h);
        ui.set_min_height(wanted);
        if sigs.is_empty() {
            ui.label(
                egui::RichText::new("(no signals yet — run a simulation)")
                    .weak()
                    .small(),
            );
            return;
        }
        // ScrollArea caps the height at 80 % of the screen so the
        // popup never spills past the window. `auto_shrink: true`
        // for height — the popup itself only grows as tall as it
        // needs. `false` for width so long names don't trigger a
        // horizontal scrollbar.
        let max_h = ui.ctx().screen_rect().height() * 0.8;
        egui::ScrollArea::vertical()
            .max_height(max_h)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (entity, path) in &sigs {
                    if ui.button(path).clicked() {
                        let payload =
                            lunco_viz::kinds::canvas_plot_node::PlotNodeData {
                                entity: entity.to_bits(),
                                signal_path: path.clone(),
                                title: String::new(),
                            };
                        let data = serde_json::to_value(&payload)
                            .unwrap_or_default();
                        let active_doc = active_doc_from_world(world);
                        let mut state =
                            world.resource_mut::<CanvasDiagramState>();
                        let docstate = state.get_mut(active_doc);
                        let scene = &mut docstate.canvas.scene;
                        let id = scene.alloc_node_id();
                        // 60×40 default size in canvas world coords;
                        // anchor top-left at the click point so the
                        // plot appears where the menu opened.
                        scene.insert_node(lunco_canvas::scene::Node {
                            id,
                            rect: lunco_canvas::Rect::from_min_max(
                                click_world,
                                lunco_canvas::Pos::new(
                                    click_world.x + 60.0,
                                    click_world.y + 40.0,
                                ),
                            ),
                            kind: lunco_viz::kinds::canvas_plot_node::PLOT_NODE_KIND
                                .into(),
                            data,
                            ports: Vec::new(),
                            label: String::new(),
                            origin: None,
                        });
                        ui.close();
                    }
                }
            });
    });
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
/// active doc threaded through: resolve it from the Workspace session.
/// Kept inline so callers outside the main render flow don't grow a
/// parameter just to pass a one-line lookup.
fn active_doc_from_world(world: &World) -> Option<lunco_doc::DocumentId> {
    world
        .resource::<lunco_workbench::WorkspaceResource>()
        .active_document
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
    theme: &lunco_theme::Theme,
) {
    let card_w = 340.0;
    let card_h = 84.0;
    let card_rect = egui::Rect::from_center_size(
        canvas_rect.center(),
        egui::vec2(card_w, card_h),
    );
    let painter = ui.painter();
    let shadow = {
        let b = theme.colors.base;
        egui::Color32::from_rgba_unmultiplied(b.r(), b.g(), b.b(), 100)
    };
    painter.rect_filled(
        card_rect.translate(egui::vec2(0.0, 3.0)),
        8.0,
        shadow,
    );
    painter.rect_filled(card_rect, 8.0, theme.colors.surface0);
    painter.rect_stroke(
        card_rect,
        8.0,
        egui::Stroke::new(1.0, theme.colors.surface2),
        egui::StrokeKind::Outside,
    );
    let t = ui.ctx().input(|i| i.time) as f32;
    let spinner_center = egui::pos2(card_rect.min.x + 28.0, card_rect.center().y);
    let accent = theme.tokens.accent;
    for i in 0..3 {
        let phase = (t * 2.5 - i as f32 * 0.4).rem_euclid(std::f32::consts::TAU);
        let alpha = ((phase.sin() * 0.5 + 0.5) * 255.0) as u8;
        let col = egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), alpha);
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
        theme.tokens.text,
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
        theme.tokens.text_subdued,
    );
    // Animating — request repaint so the spinner moves smoothly.
    ui.ctx().request_repaint();
}

// ─── Loading / projection overlay ──────────────────────────────────

/// Small "Projecting…" card centred on the canvas while an
/// `AsyncComputeTaskPool` projection task is in flight. Includes
/// a rotating dot so users can see the UI is responsive.
fn render_projecting_overlay(ui: &mut egui::Ui, canvas_rect: egui::Rect, theme: &lunco_theme::Theme) {
    let card_w = 260.0;
    let card_h = 72.0;
    let card_rect = egui::Rect::from_center_size(
        canvas_rect.center(),
        egui::vec2(card_w, card_h),
    );
    let painter = ui.painter();
    let shadow = {
        let b = theme.colors.base;
        egui::Color32::from_rgba_unmultiplied(b.r(), b.g(), b.b(), 90)
    };
    painter.rect_filled(
        card_rect.translate(egui::vec2(0.0, 3.0)),
        8.0,
        shadow,
    );
    painter.rect_filled(card_rect, 8.0, theme.colors.surface0);
    painter.rect_stroke(
        card_rect,
        8.0,
        egui::Stroke::new(1.0, theme.colors.surface2),
        egui::StrokeKind::Outside,
    );

    // Animated spinner — three dots pulsing in sequence via
    // `ctx.input(|i| i.time)`. Frame-rate independent.
    let t = ui.ctx().input(|i| i.time) as f32;
    let spinner_center = egui::pos2(card_rect.min.x + 28.0, card_rect.center().y);
    let accent = theme.tokens.accent;
    for i in 0..3 {
        let phase = (t * 2.5 - i as f32 * 0.4).rem_euclid(std::f32::consts::TAU);
        let alpha = ((phase.sin() * 0.5 + 0.5) * 255.0) as u8;
        let col = egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), alpha);
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
        theme.tokens.text,
    );
}

// ─── Empty-diagram summary ──────────────────────────────────────────

/// When the canvas scene has no nodes — common for equation-only
/// leaf models (Battery, RocketEngine, BouncyBall, SpringMass) and
/// MSL building blocks (Integrator, Resistor, Inertia) — paint a
/// "data sheet" card in the centre of the canvas. Treats the class
/// as a first-class display object instead of leaving the user
/// staring at the blank grid.
///
/// Card layout:
/// 1. **Hero strip** — the class's authored `Icon(graphics={...})`
///    annotation rendered via [`crate::icon_paint::paint_graphics`].
///    For classes without one, a stylised type-badge (M / B / C / …).
/// 2. **Heading** — class name + type label.
/// 3. **Symbol bands** — named parameters / inputs / outputs (top 6
///    each). Names beat counts: "tau, J, c" tells the user what the
///    model is for; "3 parameters" doesn't.
/// 4. **Footer counts** — equations + connect equations as a one-
///    line summary, plus a hint that points at the Text tab.
fn render_empty_diagram_overlay(
    ui: &mut egui::Ui,
    canvas_rect: egui::Rect,
    world: &mut World,
) {
    let Some(open) = world.resource::<WorkbenchState>().open_model.clone() else {
        return;
    };
    let theme = world
        .get_resource::<lunco_theme::Theme>()
        .cloned()
        .unwrap_or_else(lunco_theme::Theme::dark);
    let source = open.source.clone();
    let class_name = open
        .detected_name
        .clone()
        .unwrap_or_else(|| "(unnamed)".into());

    let counts = empty_overlay_counts_cached(source.as_ref());

    // Pull the live class info out of the document registry so we
    // can show real symbol names + (when authored) the class's own
    // `Icon` graphics. This is the same AST the canvas projector
    // already holds, so we don't pay a re-parse.
    let active_doc = active_doc_from_world(world);
    let (icon, class_type, description, param_names, input_names, output_names) =
        empty_overlay_class_info(world, active_doc, &class_name);

    crate::ui::panels::placeholder::render_centered_card(
        ui,
        canvas_rect,
        egui::vec2(440.0, 360.0),
        &theme,
        |child| {
            // ── Hero strip ────────────────────────────────────────
            // Either the authored icon or a stylised type badge.
            let hero_size = egui::vec2(120.0, 80.0);
            let (_, hero_rect) = child.allocate_space(hero_size);
            if let Some(icon) = &icon {
                crate::icon_paint::paint_graphics(
                    child.painter(),
                    hero_rect,
                    icon.coordinate_system,
                    &icon.graphics,
                );
            } else {
                paint_class_type_badge(
                    child.painter(),
                    hero_rect,
                    class_type.unwrap_or("model"),
                    &theme,
                );
            }
            child.add_space(8.0);

            // ── Class name + type label ───────────────────────────
            child.label(
                egui::RichText::new(&class_name)
                    .strong()
                    .size(15.0)
                    .color(theme.text_heading()),
            );
            if let Some(t) = class_type {
                child.label(
                    egui::RichText::new(t)
                        .size(10.5)
                        .italics()
                        .color(theme.text_muted()),
                );
            }
            if let Some(desc) = &description {
                child.add_space(4.0);
                child.label(
                    egui::RichText::new(desc)
                        .size(11.0)
                        .color(theme.tokens.text),
                );
            }
            child.add_space(8.0);
            child.separator();
            child.add_space(6.0);

            // ── Named symbol bands ───────────────────────────────
            paint_symbol_band(child, "Parameters", &param_names, counts.params, &theme);
            paint_symbol_band(child, "Inputs", &input_names, counts.inputs, &theme);
            paint_symbol_band(child, "Outputs", &output_names, counts.outputs, &theme);

            child.add_space(6.0);
            child.label(
                egui::RichText::new(format!(
                    "{} equations · {} connect equations",
                    counts.equations, counts.connects,
                ))
                .small()
                .color(theme.text_muted()),
            );
            child.add_space(4.0);
            child.label(
                egui::RichText::new("→ Switch to the Text tab to read / edit the source.")
                    .italics()
                    .size(10.0)
                    .color(theme.text_muted()),
            );
        },
    );
}

/// Pull human-friendly info about the active class: authored Icon,
/// type keyword (`model`/`block`/…), description string, and the top
/// few parameter / input / output names. Falls back to `None`/empty
/// vectors silently when the registry doesn't have the doc.
fn empty_overlay_class_info(
    world: &mut World,
    doc_id: Option<lunco_doc::DocumentId>,
    class_name: &str,
) -> (
    Option<crate::annotations::Icon>,
    Option<&'static str>,
    Option<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
) {
    let Some(doc) = doc_id else {
        return (None, None, None, vec![], vec![], vec![]);
    };
    let registry = world.resource::<ModelicaDocumentRegistry>();
    let Some(host) = registry.host(doc) else {
        return (None, None, None, vec![], vec![], vec![]);
    };
    let document = host.document();
    let ast_arc = match document.ast().result.as_ref() {
        Ok(a) => a.clone(),
        Err(_) => return (None, None, None, vec![], vec![], vec![]),
    };

    // Locate the class. Prefer an exact name match; fall back to the
    // first non-package class (matches `extract_model_name`).
    let class_def = locate_class(&ast_arc, class_name);
    let Some(class) = class_def else {
        return (None, None, None, vec![], vec![], vec![]);
    };

    use rumoca_session::parsing::ast::Causality;
    use rumoca_session::parsing::ClassType;

    let icon = crate::annotations::extract_icon(&class.annotation);
    let class_type = match class.class_type {
        ClassType::Model => Some("model"),
        ClassType::Block => Some("block"),
        ClassType::Class => Some("class"),
        ClassType::Connector => Some("connector"),
        ClassType::Record => Some("record"),
        ClassType::Type => Some("type"),
        ClassType::Package => Some("package"),
        ClassType::Function => Some("function"),
        ClassType::Operator => Some("operator"),
    };
    let description: Option<String> = class
        .description
        .iter()
        .next()
        .map(|t| t.text.as_ref().trim_matches('"').to_string())
        .filter(|s| !s.is_empty());

    let mut params = Vec::new();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for (name, comp) in class.components.iter() {
        use rumoca_session::parsing::ast::Variability;
        if matches!(comp.variability, Variability::Parameter(_)) {
            params.push(name.clone());
        }
        match comp.causality {
            Causality::Input(_) => inputs.push(name.clone()),
            Causality::Output(_) => outputs.push(name.clone()),
            _ => {}
        }
    }

    (icon, class_type, description, params, inputs, outputs)
}

/// Extract the `Diagram(graphics={...})` annotation for the target
/// class — full-qualified drill-in target, or the first non-package
/// class when no drill-in is active. Used by the background
/// decoration layer to paint MSL-style diagram callouts (labelled
/// regions, accent text) behind the nodes.
fn diagram_annotation_for_target(
    ast: &rumoca_session::parsing::ast::StoredDefinition,
    target: Option<&str>,
) -> Option<crate::annotations::Diagram> {
    // Resolve the target class by qualified path walk (supports the
    // MSL `Modelica.Blocks.Examples.PID_Controller` style). For `None`
    // targets fall back to the first non-package class, matching the
    // workbench's default active-class picker.
    let class = if let Some(qualified) = target {
        walk_qualified(ast, qualified)
    } else {
        use rumoca_session::parsing::ClassType;
        ast.classes
            .iter()
            .find(|(_, c)| !matches!(c.class_type, ClassType::Package))
            .map(|(_, c)| c)
    };
    class.and_then(|c| crate::annotations::extract_diagram(&c.annotation))
}

/// Walk a dotted qualified class path through `ast.classes` into
/// nested `class.classes`. Returns the deepest matching class, if any.
fn walk_qualified<'a>(
    ast: &'a rumoca_session::parsing::ast::StoredDefinition,
    qualified: &str,
) -> Option<&'a rumoca_session::parsing::ast::ClassDef> {
    let mut segments = qualified.split('.');
    let first = segments.next()?;
    let mut current = ast.classes.iter().find(|(n, _)| n.as_str() == first).map(|(_, c)| c)?;
    for seg in segments {
        current = current.classes.get(seg)?;
    }
    Some(current)
}

/// Find a class by short name in the AST — top-level first, then one
/// level of nested classes (the same scope `register_local_class`
/// uses for the Twin Browser).
fn locate_class<'a>(
    ast: &'a rumoca_session::parsing::ast::StoredDefinition,
    name: &str,
) -> Option<&'a rumoca_session::parsing::ast::ClassDef> {
    if let Some((_, c)) = ast.classes.iter().find(|(n, _)| n.as_str() == name) {
        return Some(c);
    }
    for (_, top) in ast.classes.iter() {
        if let Some(c) = top.classes.get(name) {
            return Some(c);
        }
    }
    // Final fallback: first non-package class (matches the workbench's
    // "active class on first open" picker).
    use rumoca_session::parsing::ClassType;
    ast.classes
        .iter()
        .find(|(_, c)| !matches!(c.class_type, ClassType::Package))
        .map(|(_, c)| c)
}

/// Render a row showing a symbol band (e.g. "Parameters: tau, J, c
/// + 3 more"). When the names list is empty, falls through to "—".
fn paint_symbol_band(
    ui: &mut egui::Ui,
    label: &str,
    names: &[String],
    total: usize,
    theme: &lunco_theme::Theme,
) {
    if total == 0 && names.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{label}:"))
                .small()
                .color(theme.text_muted()),
        );
        let shown = names.iter().take(6).cloned().collect::<Vec<_>>().join(", ");
        let suffix = if total > shown.len() && total > names.len().min(6) && names.len() > 6 {
            format!(" + {} more", total - 6)
        } else {
            String::new()
        };
        let display = if shown.is_empty() {
            format!("({total})")
        } else {
            format!("{shown}{suffix}")
        };
        ui.monospace(
            egui::RichText::new(display)
                .small()
                .color(theme.tokens.accent),
        );
    });
}

/// Stylised type badge used as the hero when a class has no authored
/// `Icon` annotation. A centred coloured pill with a single uppercase
/// letter — matches the [`crate::ui::browser_section`] type-badge
/// palette so the canvas hero and the browser row read as the same
/// "this is a model" affordance.
fn paint_class_type_badge(
    painter: &egui::Painter,
    rect: egui::Rect,
    type_name: &str,
    theme: &lunco_theme::Theme,
) {
    let letter = match type_name {
        "model" => "M",
        "block" => "B",
        "class" => "C",
        "connector" => "X",
        "record" => "R",
        "type" => "T",
        "package" => "P",
        "function" => "F",
        _ => "?",
    };
    let bg = theme.class_badge_bg_by_keyword(type_name);
    let pill_w = rect.width().min(rect.height() * 1.4);
    let pill_h = rect.height().min(120.0);
    let pill = egui::Rect::from_center_size(rect.center(), egui::vec2(pill_w, pill_h));
    painter.rect_filled(pill, 16.0, bg);
    painter.text(
        pill.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        egui::FontId::proportional(pill_h * 0.55),
        theme.class_badge_fg(),
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

/// User-facing canvas snap settings, read each frame by the canvas
/// render path and pushed onto [`lunco_canvas::Canvas::snap`]. Off by
/// default; the Settings menu flips `enabled` and picks the step.
///
/// Step is in Modelica world units (not screen pixels) so the visible
/// grid spacing stays constant across zooms. Typical choices for the
/// standard `{{-100,-100},{100,100}}` diagram coord system:
///   * `2` — fine (matches common MSL placement granularity)
///   * `5` — medium
///   * `10` — coarse (matches typical integer placements in MSL)
#[derive(bevy::prelude::Resource)]
pub struct CanvasSnapSettings {
    pub enabled: bool,
    pub step: f32,
}

impl Default for CanvasSnapSettings {
    fn default() -> Self {
        // On by default. Step = 5 Modelica units — the OMEdit
        // default and the value most MSL example placements are
        // authored to (common placement extents are multiples of 5
        // or 10). Fine enough to reach typical target positions,
        // coarse enough that every drag produces a visibly
        // different "tick" as the icon crosses grid lines.
        Self {
            enabled: true,
            step: 5.0,
        }
    }
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

/// Tab-to-task binding for duplicate-to-workspace operations whose
/// bg parse hasn't finished yet. The parse goes off the UI thread
/// because a naïve `allocate_with_origin` on a multi-KB source
/// re-runs rumoca synchronously — locked the workbench for seconds
/// in debug builds, which users (correctly) called a bug:
/// *"no operations like that must be in UI thread"*.
///
/// Same shape as [`DrillInLoads`]: the bg task returns a fully-built
/// [`ModelicaDocument`], the driver system installs it into the
/// registry via `install_prebuilt`. Cleared on install and on
/// document removal.
#[derive(bevy::prelude::Resource, Default)]
pub struct DuplicateLoads {
    pending: std::collections::HashMap<
        lunco_doc::DocumentId,
        DuplicateBinding,
    >,
}

pub struct DuplicateBinding {
    pub display_name: String,
    pub origin_short: String,
    pub started: std::time::Instant,
    pub task: bevy::tasks::Task<crate::document::ModelicaDocument>,
}

impl DuplicateLoads {
    pub fn is_loading(&self, doc: lunco_doc::DocumentId) -> bool {
        self.pending.contains_key(&doc)
    }
    pub fn detail(&self, doc: lunco_doc::DocumentId) -> Option<&str> {
        self.pending.get(&doc).map(|b| b.display_name.as_str())
    }
    pub fn progress(&self, doc: lunco_doc::DocumentId) -> Option<(&str, f32)> {
        self.pending
            .get(&doc)
            .map(|b| (b.display_name.as_str(), b.started.elapsed().as_secs_f32()))
    }
    pub fn insert(
        &mut self,
        doc: lunco_doc::DocumentId,
        binding: DuplicateBinding,
    ) {
        self.pending.insert(doc, binding);
    }
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
/// Bevy system: poll pending duplicate bg tasks; `install_prebuilt`
/// the fully-built document into the registry when ready. Same
/// shape as [`drive_drill_in_loads`] but for the `Duplicate to
/// Workspace` flow.
pub fn drive_duplicate_loads(
    mut loads: bevy::prelude::ResMut<DuplicateLoads>,
    mut registry: bevy::prelude::ResMut<ModelicaDocumentRegistry>,
) {
    use bevy::prelude::*;
    let doc_ids: Vec<lunco_doc::DocumentId> = loads.pending.keys().copied().collect();
    for doc_id in doc_ids {
        let Some(binding) = loads.pending.get_mut(&doc_id) else {
            continue;
        };
        let Some(doc) = futures_lite::future::block_on(
            futures_lite::future::poll_once(&mut binding.task),
        ) else {
            continue;
        };
        let dup_display_name = binding.display_name.clone();
        let origin_short = binding.origin_short.clone();
        loads.pending.remove(&doc_id);
        registry.install_prebuilt(doc_id, doc);
        info!(
            "[CanvasDiagram] duplicate: installed `{}` (from `{}`)",
            dup_display_name, origin_short,
        );
        // Pre-warm the MSL inheritance chain on a dedicated thread so
        // the projection finds inherited connectors. Same pattern as
        // the drill-in path. The duplicated copy carries `within
        // <origin package>;` so the within-prefixed qualified path
        // (e.g. `Modelica.Blocks.Continuous.PIDCopy`) gives the
        // scope-chain resolver enough context to walk up to
        // `Modelica.Blocks.Interfaces.SISO`.
        if let Some(host) = registry.host(doc_id) {
            if let Some(ast) = host.document().ast().result.as_ref().ok() {
                let within_prefix = ast
                    .within
                    .as_ref()
                    .map(|w| w.to_string())
                    .unwrap_or_default();
                let qpath = if within_prefix.is_empty() {
                    dup_display_name.clone()
                } else {
                    format!("{within_prefix}.{dup_display_name}")
                };
                if let Some(class) =
                    crate::diagram::find_class_by_qualified_name(ast, &qpath)
                {
                    let bases: Vec<String> = class
                        .extends
                        .iter()
                        .map(|e| e.base_name.to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !bases.is_empty() {
                        std::thread::spawn(move || {
                            crate::class_cache::prewarm_extends_chain(&qpath, &bases);
                        });
                    }
                }
            }
        }
    }
}

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
            // Pre-warm the MSL inheritance chain on a dedicated thread
            // so the projection task (which uses the cache-only
            // resolver to avoid stalling its own worker pool) finds
            // base classes already loaded.
            if let Some(ast) = entry.ast.ast() {
                if let Some(class) =
                    crate::diagram::find_class_by_qualified_name(ast, &qualified)
                {
                    let bases: Vec<String> = class
                        .extends
                        .iter()
                        .map(|e| e.base_name.to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    let qpath = qualified.clone();
                    std::thread::spawn(move || {
                        crate::class_cache::prewarm_extends_chain(&qpath, &bases);
                    });
                }
            }
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
pub fn drill_into_class(world: &mut World, qualified: &str) {
    // Try MSL paths first (resolves Modelica.* and any other MSL-rooted
    // qualified path). Fallback: scan the open document registry for a
    // doc whose AST contains the requested class — handles non-MSL
    // user-opened files (e.g. `assets/models/AnnotatedRocketStage.mo`)
    // where the qualified name lives only in a workspace document.
    let file_path = crate::class_cache::resolve_msl_class_path(qualified)
        .or_else(|| crate::class_cache::locate_msl_file(qualified));
    if let Some(file_path) = file_path {
        open_drill_in_tab(world, qualified, &file_path);
        return;
    }
    // Open-document fallback: find a host whose parsed AST resolves the
    // qualified path. Reuse its tab + just set the drill-in class.
    let target_doc: Option<lunco_doc::DocumentId> = {
        let registry = world.resource::<ModelicaDocumentRegistry>();
        registry.iter().find_map(|(doc_id, host)| {
            host.document().ast().ast().and_then(|ast| {
                crate::diagram::find_class_by_qualified_name(ast, qualified)
                    .map(|_| doc_id)
            })
        })
    };
    if let Some(doc_id) = target_doc {
        // Switch focus to this doc's tab and record the drilled-in
        // class so the canvas projection scopes itself.
        if let Some(mut tabs) =
            world.get_resource_mut::<crate::ui::panels::model_view::ModelTabs>()
        {
            if let Some(tab) = tabs.get_mut(doc_id) {
                tab.view_mode = crate::ui::panels::model_view::ModelViewMode::Canvas;
            }
        }
        if let Some(mut names) =
            world.get_resource_mut::<DrilledInClassNames>()
        {
            names.set(doc_id, qualified.to_string());
        }
        if let Some(mut workspace) =
            world.get_resource_mut::<lunco_workbench::WorkspaceResource>()
        {
            workspace.active_document = Some(doc_id);
        }
        bevy::log::info!(
            "[CanvasDiagram] drill-in: focused open doc for `{}`",
            qualified,
        );
        return;
    }
    bevy::log::warn!(
        "[CanvasDiagram] drill-in: could not locate `{}` (no MSL match, no open doc with that class)",
        qualified
    );
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
    // Active doc from the Workspace session; `open_model.detected_name`
    // is read as a display-cache fallback when the registry AST hasn't
    // caught up yet. Both paths are optional — the caller tolerates
    // `(None, None)` by deferring.
    let Some(doc_id) = world
        .resource::<lunco_workbench::WorkspaceResource>()
        .active_document
    else {
        return (None, None);
    };
    let open = world.resource::<WorkbenchState>().open_model.as_ref();
    let class = world
        .resource::<ModelicaDocumentRegistry>()
        .host(doc_id)
        .and_then(|h| {
            h.document()
                .ast()
                .ast()
                .and_then(|s| s.classes.keys().next().cloned())
        })
        .or_else(|| open.and_then(|o| o.detected_name.clone()));
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
                // Use the node's actual icon extent — `Placement::at`
                // hardcodes 20×20, which silently shrinks (or grows)
                // every dragged component back to the default size on
                // re-projection. Read the live `node.rect` instead so
                // the new placement preserves whatever size the icon
                // already has on screen (canvas world coords are 1:1
                // with Modelica units, just Y-flipped).
                let icon_w = node.rect.width().max(1.0);
                let icon_h = node.rect.height().max(1.0);
                let m = coords::canvas_min_to_modelica_center(*new_min, icon_w, icon_h);
                ops.push(ModelicaOp::SetPlacement {
                    class: class.to_string(),
                    name,
                    placement: Placement {
                        x: m.x,
                        y: m.y,
                        width: icon_w,
                        height: icon_h,
                    },
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
/// Public re-export of the canvas's op applier so reflect-registered
/// commands (`MoveComponent`, etc.) can dispatch the same SetPlacement
/// pipeline the mouse drag uses — keeps undo/redo + source rewriting
/// consistent across UI-driven and API-driven edits.
pub fn apply_ops_public(
    world: &mut World,
    doc_id: lunco_doc::DocumentId,
    ops: Vec<ModelicaOp>,
) {
    apply_ops(world, doc_id, ops);
}

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
        // User-facing explanation. The tab title already shows a 👁
        // read-only chip, but silent drops still confuse people on
        // first edit attempt — surface the "why nothing happened" in
        // the Diagnostics banner so it's unmissable. The duplicate
        // action clears this on its own (Duplicate → new Untitled →
        // read_only=false → future ops write through cleanly).
        if let Some(mut ws) = world.get_resource_mut::<WorkbenchState>() {
            ws.compilation_error = Some(
                "This is a read-only library model. \
                 Use File → Duplicate to Workspace \
                 (or the Duplicate button in the tab header) \
                 to create an editable copy of it, then try again."
                    .to_string(),
            );
        }
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

/// Observer for [`crate::ui::commands::AutoArrangeDiagram`].
///
/// Assigns every component of the active class a grid position from
/// the current [`crate::ui::panels::diagram::DiagramAutoLayoutSettings`]
/// `arrange_*` parameters and emits a batch of `SetPlacement` ops.
///
/// Iterates the canvas scene (not the AST) so the order matches what
/// the user sees. Each op is separately undo-able via Ctrl+Z.
pub fn on_auto_arrange_diagram(
    trigger: On<crate::ui::commands::AutoArrangeDiagram>,
    mut commands: Commands,
) {
    let raw = trigger.event().doc;
    // Observers can't take `&mut World` in Bevy 0.18. Defer the real
    // work to an exclusive command — same mutations, just queued to
    // run at the next command-flush boundary.
    commands.queue(move |world: &mut World| {
        // `doc = 0` = API / script default = "the tab the user is
        // looking at right now". Resolve from `WorkbenchState.open_model`
        // so the LunCo API can fire the command without tracking ids.
        let doc_id = if raw == 0 {
            match active_doc_from_world(world) {
                Some(d) => d,
                None => {
                    bevy::log::warn!(
                        "[CanvasDiagram] Auto-Arrange: no active doc"
                    );
                    return;
                }
            }
        } else {
            lunco_doc::DocumentId::new(raw)
        };
        auto_arrange_now(world, doc_id);
    });
}

fn auto_arrange_now(world: &mut World, doc_id: lunco_doc::DocumentId) {
    let Some(class) = active_class_for_doc(world, doc_id) else {
        return;
    };
    let layout = world
        .get_resource::<crate::ui::panels::diagram::DiagramAutoLayoutSettings>()
        .cloned()
        .unwrap_or_default();
    // Capture each node's `origin` (Modelica instance name) AND
    // its existing rect size so Auto-Arrange can preserve per-node
    // extents — the prior `Placement::at` form squashed every icon
    // back to the default 20×20, undoing the user's authored sizes.
    let mut named_with_size: Vec<(String, f32, f32)> = {
        let Some(state) = world.get_resource::<CanvasDiagramState>() else {
            return;
        };
        let docstate = state.get(Some(doc_id));
        docstate
            .canvas
            .scene
            .nodes()
            .filter_map(|(_, n)| {
                let origin = n.origin.clone()?;
                Some((origin, n.rect.width().max(1.0), n.rect.height().max(1.0)))
            })
            .collect()
    };
    // Stable sort + dedup by name: the original `dedup()` only
    // removed adjacent duplicates, which the unsorted scene order
    // didn't guarantee.
    named_with_size.sort_by(|a, b| a.0.cmp(&b.0));
    named_with_size.dedup_by(|a, b| a.0 == b.0);
    if named_with_size.is_empty() {
        return;
    }

    let cols = layout.cols.max(1);
    let dx = layout.spacing_x;
    let dy = layout.spacing_y;
    let stagger = dx * layout.row_stagger;
    let ops: Vec<ModelicaOp> = named_with_size
        .into_iter()
        .enumerate()
        .map(|(idx, (name, w, h))| {
            let row = idx / cols;
            let col = idx % cols;
            let row_shift = if row % 2 == 1 { stagger } else { 0.0 };
            // Canvas world coords (+Y down). Convert to Modelica
            // centre (+Y up) via the shared helper so the ops emit
            // the same coord frame a drag would.
            let wx = col as f32 * dx + row_shift;
            let wy = row as f32 * dy;
            let m = coords::canvas_min_to_modelica_center(
                lunco_canvas::Pos::new(wx, wy),
                w,
                h,
            );
            ModelicaOp::SetPlacement {
                class: class.clone(),
                name,
                placement: Placement {
                    x: m.x,
                    y: m.y,
                    width: w,
                    height: h,
                },
            }
        })
        .collect();
    if ops.is_empty() {
        return;
    }
    bevy::log::info!(
        "[CanvasDiagram] Auto-Arrange: emitting {} SetPlacement ops",
        ops.len()
    );
    apply_ops(world, doc_id, ops);
}

/// Resolve the active class name for an Auto-Arrange target. Prefers
/// the drilled-in class name (for MSL drill-in tabs); falls back to
/// the open document's detected model name.
fn active_class_for_doc(world: &mut World, doc_id: lunco_doc::DocumentId) -> Option<String> {
    if let Some(m) = world.get_resource::<DrilledInClassNames>() {
        if let Some(c) = m.get(doc_id) {
            return Some(c.to_string());
        }
    }
    world
        .get_resource::<WorkbenchState>()
        .and_then(|ws| ws.open_model.as_ref())
        .and_then(|o| o.detected_name.clone())
}