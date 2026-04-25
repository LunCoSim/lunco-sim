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

/// Per-frame snapshot of model inputs that are exposed for direct
/// in-canvas control (Dashboard-style sliders / knobs). Indexed by
/// the same fully-qualified instance path as [`NodeStateSnapshot`]
/// (e.g. `"valve.opening"`). Includes the declared `min`/`max`
/// bounds so widgets can clamp without a separate lookup.
#[derive(Debug, Default, Clone)]
pub struct InputControlSnapshot {
    /// `"<instance>.<input>" -> (current_value, min, max)`.
    pub inputs: std::collections::HashMap<String, (f64, Option<f64>, Option<f64>)>,
}

pub fn stash_input_control_snapshot(ctx: &egui::Context, snapshot: InputControlSnapshot) {
    ctx.data_mut(|d| d.insert_temp(input_control_snapshot_id(), Arc::new(snapshot)));
}

pub fn fetch_input_control_snapshot(ctx: &egui::Context) -> Arc<InputControlSnapshot> {
    ctx.data(|d| d.get_temp::<Arc<InputControlSnapshot>>(input_control_snapshot_id()))
        .unwrap_or_default()
}

/// Push a write request from an in-canvas control widget into the
/// per-frame queue. The host drains it after the canvas finishes
/// rendering and applies the changes to the model. Keys are the
/// fully-qualified input names; multiple writes during one frame
/// last-write-wins.
pub fn queue_input_write(ctx: &egui::Context, name: &str, value: f64) {
    let queue: Arc<std::sync::Mutex<std::collections::HashMap<String, f64>>> = ctx.data_mut(|d| {
        if let Some(existing) = d.get_temp(input_writes_id()) {
            existing
        } else {
            let fresh: Arc<std::sync::Mutex<std::collections::HashMap<String, f64>>> =
                Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
            d.insert_temp(input_writes_id(), fresh.clone());
            fresh
        }
    });
    if let Ok(mut guard) = queue.lock() {
        guard.insert(name.to_string(), value);
    };
}

/// Drain the per-frame input-write queue. Called by the host once
/// per render pass after the canvas is done; returns the pending
/// writes for the host to apply to its `ModelicaModel.inputs`
/// (which the worker forwards to `SimStepper::set_input`).
pub fn drain_input_writes(ctx: &egui::Context) -> Vec<(String, f64)> {
    let queue: Option<Arc<std::sync::Mutex<std::collections::HashMap<String, f64>>>> =
        ctx.data(|d| d.get_temp(input_writes_id()));
    let Some(queue) = queue else { return Vec::new() };
    let Ok(mut guard) = queue.lock() else { return Vec::new() };
    guard.drain().collect()
}

fn input_control_snapshot_id() -> egui::Id {
    egui::Id::new("lunco_viz_input_control_snapshot")
}

fn input_writes_id() -> egui::Id {
    egui::Id::new("lunco_viz_input_write_queue")
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
        // The plot fills exactly its `node.rect` (transformed to
        // screen). No artificial MIN clamp — the resize handle hits
        // the actual rect.max corner, so any visual MIN would
        // decouple the grip from where it appears. Users zoom in
        // when they need the plot bigger.
        // Cull only when fully outside the canvas widget. Sub-pixel
        // rects DO still paint — `egui` rounds them to 1px which
        // keeps the plot visible as a marker dot at extreme zoom-out
        // (rather than vanishing entirely as before).
        if !ctx.ui.max_rect().intersects(egui_rect) {
            return;
        }
        if egui_rect.width() < 1.0 || egui_rect.height() < 1.0 {
            // Degenerate (zero-area) rect — paint a 1×1 marker
            // anchored at the world centre so the user can still
            // see the plot exists and zoom in to operate on it.
            let centre = egui_rect.center();
            let marker = egui::Rect::from_center_size(
                centre,
                egui::vec2(2.0, 2.0),
            );
            let theme = lunco_canvas::theme::current(ctx.ui.ctx());
            ctx.ui.painter().rect_filled(marker, 1.0, theme.overlay_stroke);
            return;
        }

        let theme = lunco_canvas::theme::current(ctx.ui.ctx());
        let stroke = if selected {
            egui::Stroke::new(2.0, theme.selection_outline)
        } else {
            egui::Stroke::new(1.0, theme.overlay_stroke)
        };
        // Plot card uses a near-black fill so it always pops
        // against the dark-grey canvas background — small +12 RGB
        // bump from `overlay_fill` was visually invisible at small
        // sizes (canvas bg is around the same brightness).
        let card_fill = egui::Color32::from_rgb(8, 12, 18);
        ctx.ui.painter().rect_filled(egui_rect, 6.0, card_fill);
        // `Inside` so the stroke stays *within* the node's rect —
        // `Outside` would visually extend the plot 1-2 px past
        // node.rect on every side, decoupling the apparent size
        // from the resize handle's hit position.
        ctx.ui
            .painter()
            .rect_stroke(egui_rect, 6.0, stroke, egui::StrokeKind::Inside);

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

        let inner_rect = egui_rect.shrink(4.0);
        let mut child = ctx.ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        // Hard-clip the child UI so anything inside (label text,
        // egui_plot legend / axes) never paints past the node's
        // rect. egui_plot otherwise prefers its `min_size` (~96 px)
        // and overflows when the node is smaller than that.
        child.set_clip_rect(inner_rect);
        let label_h = if show_label {
            // Title row with hover hint — shows the full
            // signal-binding path (which may be truncated in the
            // visible label) plus a hint sentence so users can
            // identify what they're looking at without opening
            // the inspector. egui's `Label::sense(hover)` lets us
            // attach a tooltip without affecting layout.
            let label_resp = child.add(
                egui::Label::new(
                    egui::RichText::new(title)
                        .small()
                        .color(theme.overlay_text),
                )
                .sense(egui::Sense::hover()),
            );
            if label_resp.hovered() {
                label_resp.on_hover_ui(|ui| {
                    ui.label(
                        egui::RichText::new(&self.data.signal_path)
                            .strong()
                            .monospace(),
                    );
                    ui.label(
                        egui::RichText::new(
                            "in-canvas plot — bound to a single \
                             scalar signal; drag corner to resize",
                        )
                        .small()
                        .weak(),
                    );
                });
            }
            14.0_f32
        } else {
            0.0
        };
        let color = crate::signal::color_for_signal(&self.data.signal_path);
        // Explicit width/height so the plot fills exactly the
        // remaining child area — no growth past the card.
        let plot_w = inner_rect.width().max(1.0);
        let plot_h = (inner_rect.height() - label_h).max(1.0);
        Plot::new(("plot_node", node.id.0))
            .width(plot_w)
            .height(plot_h)
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
