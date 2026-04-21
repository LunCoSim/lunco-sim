//! `lunco-canvas` `NodeVisual` impl that renders a live time-series
//! plot inside a scene node — Simulink-style "Scope" block.
//!
//! Plots are first-class `Scene` nodes (kind = [`PLOT_NODE_KIND`]).
//! They participate in selection / drag / resize / copy-paste / undo
//! through the same machinery component nodes use; nothing special-
//! cases them at the canvas core. Their per-node payload carries the
//! signal binding (entity + path); samples are read from a
//! per-frame snapshot the host stashes via [`stash_signal_snapshot`].
//!
//! ## Why a snapshot, not a Resource lookup
//!
//! `NodeVisual::draw` only sees `&DrawCtx` — no `World`. The host
//! therefore copies the relevant signals into an `egui` `Context`
//! data slot once per frame, and visuals read it back via
//! [`fetch_signal_snapshot`]. This keeps `lunco-canvas` ignorant of
//! Bevy entities / `SignalRegistry` while still letting plots see
//! live data without per-call resource lookups.
//!
//! ## Future kinds
//!
//! Cameras, dashboards, sticky notes — same pattern: implement
//! `NodeVisual`, register the kind from the integrating crate, store
//! per-node config in `Node::data`. The `extras` slot stays free for
//! tool-preview state.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::Entity;
use bevy_egui::egui;
use egui_plot::{Line, Plot, PlotPoints};
use lunco_canvas::{visual::DrawCtx, NodeVisual};
use lunco_canvas::scene::Node;
use serde::{Deserialize, Serialize};

/// Stable kind identifier — use this when registering with the
/// `VisualRegistry` and when authoring `Node::kind` for a plot node.
pub const PLOT_NODE_KIND: &str = "lunco.viz.plot";

/// Per-node persisted payload. Stored in `Node::data` as JSON so the
/// scene serialiser handles round-trips without knowing about plots.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlotNodeData {
    /// Bevy entity that produced the signal samples. Encoded
    /// alongside the path because `SignalRef` is `(entity, path)`-
    /// keyed; the host fills this in when constructing the node.
    /// Stored as `u64` so the JSON shape doesn't depend on Bevy
    /// internals.
    pub entity: u64,
    /// Signal path (e.g. `"P.y"`).
    pub signal_path: String,
    /// Display label. Defaults to `signal_path` when empty.
    #[serde(default)]
    pub title: String,
}

/// One sampled point — `(time, value)` — used in the per-frame
/// snapshot. Decoupled from `lunco_viz::ScalarSample` so this module
/// stays free of the rest of the viz crate's signal types.
pub type SamplePoint = [f64; 2];

/// Per-frame snapshot of the signal samples this canvas's plot
/// nodes need. Built once by the host system, stashed on the
/// `egui::Context`, read by every plot visual.
#[derive(Debug, Default, Clone)]
pub struct SignalSnapshot {
    pub samples: HashMap<(Entity, String), Vec<SamplePoint>>,
}

/// Stash a snapshot in the egui context so any plot node visual
/// drawn this frame can pull data without a `World` reference. The
/// host must call this *before* `Canvas::ui` each frame — passing an
/// empty snapshot is fine, plots just render no line.
pub fn stash_signal_snapshot(ctx: &egui::Context, snapshot: SignalSnapshot) {
    ctx.data_mut(|d| d.insert_temp(snapshot_id(), Arc::new(snapshot)));
}

/// Fetch the most recently stashed snapshot. Returns an empty
/// snapshot if the host hasn't stashed one this frame — visuals
/// degrade to "no line, label only" rather than panicking.
pub fn fetch_signal_snapshot(ctx: &egui::Context) -> Arc<SignalSnapshot> {
    ctx.data(|d| d.get_temp::<Arc<SignalSnapshot>>(snapshot_id()))
        .unwrap_or_default()
}

fn snapshot_id() -> egui::Id {
    egui::Id::new("lunco_viz_signal_snapshot")
}

/// Per-frame snapshot of every component-instance scalar value
/// (parameter / input / variable). Indexed by instance path
/// (e.g. `"R1.R"`, `"P.y"`). Stashed by the host alongside
/// [`SignalSnapshot`] so any node visual can show hover tooltips
/// or inline value badges without `World` access.
#[derive(Debug, Default, Clone)]
pub struct NodeStateSnapshot {
    /// `"<instance>.<var>" -> value`. Includes parameters, inputs,
    /// and live simulator variables — visuals filter by prefix
    /// (e.g. an icon for instance `R1` shows entries starting with
    /// `"R1."`).
    pub values: std::collections::HashMap<String, f64>,
}

pub fn stash_node_state(ctx: &egui::Context, snapshot: NodeStateSnapshot) {
    ctx.data_mut(|d| d.insert_temp(node_state_id(), Arc::new(snapshot)));
}

pub fn fetch_node_state(ctx: &egui::Context) -> Arc<NodeStateSnapshot> {
    ctx.data(|d| d.get_temp::<Arc<NodeStateSnapshot>>(node_state_id()))
        .unwrap_or_default()
}

fn node_state_id() -> egui::Id {
    egui::Id::new("lunco_viz_node_state_snapshot")
}

/// `NodeVisual` impl reconstructed by the registry from
/// `PlotNodeData`. Holds the binding only; samples come from the
/// per-frame snapshot.
pub struct PlotNodeVisual {
    pub data: PlotNodeData,
}

impl PlotNodeVisual {
    pub fn from_data(data: PlotNodeData) -> Self {
        Self { data }
    }
}

impl NodeVisual for PlotNodeVisual {
    fn draw(&self, ctx: &mut DrawCtx, node: &Node, selected: bool) {
        // Transform the world rect to screen so the plot tracks
        // pan/zoom like any other node.
        let screen_rect = ctx
            .viewport
            .world_rect_to_screen(node.rect, ctx.screen_rect);
        let egui_rect = egui::Rect::from_min_max(
            egui::pos2(screen_rect.min.x, screen_rect.min.y),
            egui::pos2(screen_rect.max.x, screen_rect.max.y),
        );
        if !ctx.ui.max_rect().intersects(egui_rect) {
            return;
        }

        let theme = lunco_canvas::theme::current(ctx.ui.ctx());
        let stroke = if selected {
            egui::Stroke::new(2.0, theme.selection_outline)
        } else {
            egui::Stroke::new(1.0, theme.overlay_stroke)
        };
        ctx.ui
            .painter()
            .rect_filled(egui_rect, 6.0, theme.overlay_fill);
        ctx.ui
            .painter()
            .rect_stroke(egui_rect, 6.0, stroke, egui::StrokeKind::Outside);

        // Bottom-right resize grip: two short diagonals so the user
        // can find the drag handle. Sized in screen pixels so the
        // grip stays usable at any zoom level.
        let grip = egui::pos2(egui_rect.max.x - 2.0, egui_rect.max.y - 2.0);
        let grip_color = theme.overlay_stroke;
        for off in [4.0_f32, 8.0_f32] {
            ctx.ui.painter().line_segment(
                [
                    egui::pos2(grip.x - off, grip.y),
                    egui::pos2(grip.x, grip.y - off),
                ],
                egui::Stroke::new(1.0, grip_color),
            );
        }

        let title = if self.data.title.is_empty() {
            &self.data.signal_path
        } else {
            &self.data.title
        };
        let entity = Entity::from_bits(self.data.entity);
        let snapshot = fetch_signal_snapshot(ctx.ui.ctx());
        let key = (entity, self.data.signal_path.clone());
        let points = snapshot.samples.get(&key).cloned().unwrap_or_default();

        // Adaptive density: when zoomed out the card is tiny and a
        // text label / axes would be larger than the chart itself.
        // Hide labels under 80×60 px, hide everything but the line
        // under 40×30 px. Symmetric with how vector design tools
        // handle thumbnail nodes.
        let card_w = egui_rect.width();
        let card_h = egui_rect.height();
        let show_label = card_w >= 80.0 && card_h >= 60.0;
        if card_w < 40.0 || card_h < 30.0 {
            // Tiny — just paint a sparkline directly into the rect,
            // no child UI.
            if !points.is_empty() {
                let color = crate::signal::color_for_signal(&self.data.signal_path);
                let (mut tmin, mut tmax, mut vmin, mut vmax) =
                    (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
                for p in &points {
                    tmin = tmin.min(p[0]);
                    tmax = tmax.max(p[0]);
                    vmin = vmin.min(p[1]);
                    vmax = vmax.max(p[1]);
                }
                let dt = (tmax - tmin).max(f64::EPSILON);
                let dv = (vmax - vmin).max(f64::EPSILON);
                let pts: Vec<egui::Pos2> = points
                    .iter()
                    .map(|p| {
                        egui::pos2(
                            egui_rect.min.x
                                + ((p[0] - tmin) / dt) as f32 * card_w,
                            egui_rect.max.y
                                - ((p[1] - vmin) / dv) as f32 * card_h,
                        )
                    })
                    .collect();
                ctx.ui
                    .painter()
                    .add(egui::Shape::line(pts, egui::Stroke::new(1.5, color)));
            }
            return;
        }

        let mut child = ctx.ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui_rect.shrink(4.0))
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        if show_label {
            child.label(
                egui::RichText::new(title)
                    .small()
                    .color(theme.overlay_text),
            );
        }
        let color = crate::signal::color_for_signal(&self.data.signal_path);
        Plot::new(("plot_node", node.id.0))
            .show_axes([false, false])
            .show_grid(false)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .show(&mut child, |plot_ui| {
                if !points.is_empty() {
                    plot_ui.line(
                        Line::new("", PlotPoints::from(points)).color(color),
                    );
                }
            });
    }

    fn debug_name(&self) -> &str {
        PLOT_NODE_KIND
    }
}

/// Convenience: register this kind with a `VisualRegistry`.
/// Domain crates that wire `lunco-canvas` + `lunco-viz` call this
/// once at plugin-build time.
pub fn register(reg: &mut lunco_canvas::VisualRegistry) {
    reg.register_node_kind(PLOT_NODE_KIND, |data: &serde_json::Value| {
        let payload: PlotNodeData = serde_json::from_value(data.clone())
            .unwrap_or_default();
        PlotNodeVisual::from_data(payload)
    });
}
