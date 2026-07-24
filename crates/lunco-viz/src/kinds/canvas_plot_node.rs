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
use lunco_theme::ColorAlpha;
use lunco_canvas::scene::Node;
use serde::{Deserialize, Serialize};

/// Stable kind identifier — use this when registering with the
/// `VisualRegistry` and when authoring `Node::kind` for a plot node.
pub const PLOT_NODE_KIND: &str = "lunco.viz.plot";

/// How a plot tile chooses its sim entity. Two distinct policies,
/// kept as an enum (rather than two optional fields) so the type
/// system enforces "exactly one mode is active" — there is no
/// representable state for a tile that's both pinned and per-doc.
///
/// * [`PlotBinding::Pinned`] — Telemetry-bound. The user explicitly
///   chose a specific sim entity; samples come from that entity
///   only, even if other sims publish the same signal name.
/// * [`PlotBinding::Doc`] — Source-backed. The tile belongs to a
///   document, not to a specific sim. Resolved to the document's
///   currently-bound sim entity each frame via the snapshot's
///   `doc_to_entity` table; survives sim restart, tab switches, and
///   re-projection without needing a rebind op.
///
/// JSON encoding uses `#[serde(untagged)]`: discrimination is by
/// field presence, not a tag — `{ "entity": 42 }` → `Pinned`,
/// `{ "doc_id": 7 }` → `Doc`. Pre-enum scenes (which carried only
/// `entity`) read back as `Pinned` unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlotBinding {
    /// Pinned to a specific Bevy entity (Telemetry path). `entity`
    /// is the raw `Entity::to_bits()` value; `0` is the unbound
    /// sentinel (`Entity::try_from_bits(0)` returns `None`).
    Pinned { entity: u64 },
    /// Resolved at fetch time from the document's bound sim entity.
    /// `doc_id` is `DocumentId::raw()` — kept as bare `u64` so
    /// `lunco-viz` doesn't depend on `lunco-doc`.
    Doc { doc_id: u64 },
}

impl Default for PlotBinding {
    fn default() -> Self {
        // Backwards-compatible default: legacy scenes deserialised
        // without a binding land here, matching the old `entity = 0`
        // unbound behaviour.
        PlotBinding::Pinned { entity: 0 }
    }
}

impl PlotBinding {
    /// Entity bits if this binding is [`PlotBinding::Pinned`], else
    /// `None`. Used by "is this signal the currently-pinned one?"
    /// checkmark UI which only makes sense in pinned mode — a Doc-
    /// bound tile has no specific entity to compare against from
    /// the user's perspective.
    pub fn pinned_entity(&self) -> Option<u64> {
        match self {
            PlotBinding::Pinned { entity } => Some(*entity),
            PlotBinding::Doc { .. } => None,
        }
    }
}

/// Per-node persisted payload. Stored in `Node::data` as JSON so the
/// scene serialiser handles round-trips without knowing about plots.
/// The [`PlotBinding`] enum is flattened so the JSON keys stay
/// top-level (`entity` or `doc_id` alongside `signal_path`/`title`)
/// — same wire shape as before the refactor.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlotNodeData {
    /// How this tile picks its sim entity. See [`PlotBinding`].
    #[serde(flatten)]
    pub binding: PlotBinding,
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
    /// Document → currently-bound sim entity. Populated by the host
    /// from `ModelicaDocumentRegistry`. Per-doc plot tiles
    /// (`PlotNodeData.doc_id = Some(_)`) use this to recover the
    /// runtime sim entity at fetch time instead of baking it in at
    /// projection. Survives sim restart and tab switches without a
    /// re-projection — the same scene Node tracks whatever sim is
    /// active for its document right now.
    pub doc_to_entity: HashMap<u64, Entity>,
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
/// (which the worker forwards to `SimulationSession::set_input`).
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
        // Series colour from the active THEME, not a hardcoded palette.
        let theme = lunco_theme::active(ctx.ui.ctx());
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
        // Cull only when fully off-canvas. The canvas widget's
        // `set_clip_rect` (in lunco-canvas) handles the visual
        // clipping for partial overlaps, so half-visible cards still
        // render correctly without a broken-frame artefact.
        if !ctx.ui.clip_rect().intersects(egui_rect) {
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
            ctx.ui.painter().rect_filled(marker, 1.0, theme.colors.surface2);
            return;
        }

        // Hard-clip everything we paint to the canvas widget's own
        // clip rect — without this the right-most plot in a row of
        // nodes can visibly bleed into the inspector / side-panel
        // area (e.g. paint over the Telemetry panel when the plot
        // node is dragged near the right edge of the canvas).
        let canvas_clip = ctx.ui.clip_rect();
        let stroke = if selected {
            egui::Stroke::new(2.0, theme.tokens.accent)
        } else {
            egui::Stroke::new(1.0, theme.colors.surface2)
        };
        // Card fill comes from the canvas theme so the plot matches
        // the rest of the diagram nodes (no jarring near-black box
        // when the active theme is light or mid-grey).
        ctx.ui.painter().rect_filled(egui_rect, 6.0, theme.colors.surface0);
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
        let grip_color = theme.colors.surface2;
        for off in [4.0_f32, 8.0_f32] {
            ctx.ui.painter().line_segment(
                [
                    egui::pos2(grip.x - off, grip.y),
                    egui::pos2(grip.x, grip.y - off),
                ],
                egui::Stroke::new(1.0, grip_color),
            );
        }

        let title: &str = if !self.data.title.is_empty() {
            &self.data.title
        } else if !self.data.signal_path.is_empty() {
            &self.data.signal_path
        } else {
            "(unbound plot)"
        };
        // Sample lookup — dispatch on the typed binding. See
        // `PlotBinding` for the policy semantics. Either path can
        // return `None` (unbound tile, sim not running, etc.) — the
        // tile degrades to "no curve, just the title" instead of
        // panicking.
        let snapshot = fetch_signal_snapshot(ctx.ui.ctx());
        let resolved_entity: Option<Entity> = match &self.data.binding {
            PlotBinding::Pinned { entity } => Entity::try_from_bits(*entity),
            PlotBinding::Doc { doc_id } => snapshot.doc_to_entity.get(doc_id).copied(),
        };
        // Borrow the sample slice out of the (Arc-held) snapshot — it
        // lives for the whole render, so there's no need to deep-clone
        // the curve here. The single owned copy egui_plot demands is
        // made once at `PlotPoints::from` below (CQ-207: was cloned
        // twice — once on fetch, once into the plot).
        let points: &[SamplePoint] = resolved_entity
            .and_then(|e| snapshot.samples.get(&(e, self.data.signal_path.clone())))
            .map(Vec::as_slice)
            .unwrap_or(&[]);

        // Adaptive density: at extreme zoom-out we drop to a bare
        // sparkline (under 40×30 px). Above that we *always* keep
        // the title row so the chart doesn't suddenly grow when the
        // card crosses the old 80×60 threshold (that jump was
        // jarring during canvas zoom).
        let card_w = egui_rect.width();
        let card_h = egui_rect.height();
        let show_label = card_w >= 40.0 && card_h >= 30.0;
        if card_w < 40.0 || card_h < 30.0 {
            // Tiny — just paint a sparkline directly into the rect,
            // no child UI.
            if !points.is_empty() {
                let color = crate::signal::color_for_signal(&theme, &self.data.signal_path);
                let (mut tmin, mut tmax, mut vmin, mut vmax) =
                    (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
                for p in points.iter() {
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

        // Tight inset — we draw our own axis labels as overlays
        // inside the plot area, so we don't need to reserve a margin
        // for egui_plot's external axis strip.
        let inner_rect = egui_rect.shrink(2.0);
        let mut child = ctx.ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        // Hard-clip the child UI so anything inside (label text,
        // egui_plot legend / axes) never paints past the node's
        // rect *or* past the canvas widget itself. egui_plot
        // otherwise prefers its `min_size` (~96 px) and overflows
        // when the node is smaller than that. Intersecting with
        // `canvas_clip` is what stops a plot dragged near the right
        // edge from painting over the Telemetry side panel.
        child.set_clip_rect(inner_rect.intersect(canvas_clip));
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
                        .color(theme.tokens.text),
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
        let color = crate::signal::color_for_signal(&theme, &self.data.signal_path);
        // Explicit width/height so the plot fills the child area
        // *except* a small bottom-right strip reserved for the
        // canvas's resize grip. Without this reservation egui_plot
        // allocates a Response over the corner and steals the
        // mouse-down, so the user can never resize the plot node.
        let grip_reserve = 12.0_f32;
        let plot_w = (inner_rect.width() - grip_reserve).max(1.0);
        let plot_h = (inner_rect.height() - label_h - grip_reserve).max(1.0);
        // Plot rect inside the card (under the title). Used for the
        // overlay axis labels we paint directly onto the data area.
        let plot_min_y = inner_rect.min.y + label_h;
        let plot_rect = egui::Rect::from_min_max(
            egui::pos2(inner_rect.min.x, plot_min_y),
            egui::pos2(inner_rect.max.x, inner_rect.max.y),
        );
        // Adaptive chrome: only worth painting axis-labels overlay
        // when the card is large enough that they don't dominate the
        // line. Below 140×100 we keep the bare line + title.
        let show_chrome = card_w >= 140.0 && card_h >= 100.0;
        // Salt the Plot widget id by the parent UI id so the same
        // canvas NodeId in a different document/tab doesn't collide.
        // Without this, duplicating a running model can render two
        // canvases in one frame whose plot nodes share id (X, N) but
        // live under different egui layers, tripping egui's
        // "widget changed layer_id" assertion in widget_rect.rs.
        let plot = Plot::new(ctx.ui.id().with(("plot_node", node.id.0)))
            .width(plot_w)
            .height(plot_h)
            // We draw our own min/max overlays inside the chart, so
            // egui_plot's external axis strip / grid / legend are
            // turned off — this also gives us the full width/height
            // for the line itself. Title row above the plot already
            // identifies the signal, so no in-plot legend.
            .show_axes([false, false])
            .show_grid(false)
            .show_background(false)
            // Plot does not capture pointer/wheel — the canvas owns
            // pan/zoom AND the resize-handle hit test. egui_plot's
            // default `click_and_drag` sense would swallow the
            // primary-down before the canvas tool sees it, so the
            // user can never grab the bottom-right resize grip on
            // a plot node. `hover()` lets the click fall through.
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .allow_boxed_zoom(false)
            .sense(egui::Sense::hover())
            // Hover tooltip used to spit raw f64 digits — when the
            // sim diverges to 1e260 that's a 300-character wall of
            // numbers that overflows the panel into adjacent UI.
            // Route through `format_axis_value` for a compact form.
            .label_formatter({
                let var = short_name(&self.data.signal_path).to_owned();
                // egui_plot 0.36: single `HoverPosition` arg, returns `Option<String>`.
                move |pos| {
                    let p = match pos {
                        egui_plot::HoverPosition::NearDataPoint { position, .. }
                        | egui_plot::HoverPosition::Elsewhere { position } => position,
                    };
                    Some(format!("t: {} s\n{}: {}", format_axis_value(p.x), var, format_axis_value(p.y)))
                }
            });
        plot.show(&mut child, |plot_ui| {
            if !points.is_empty() {
                let line_label = if self.data.title.is_empty() {
                    self.data.signal_path.as_str()
                } else {
                    self.data.title.as_str()
                };
                plot_ui.line(
                    Line::new(line_label, PlotPoints::from(points.to_vec())).color(color),
                );
            }
        });

        // Overlay axis numbers + faint grid lines INSIDE the plot
        // rect so they read against the dark card without bleeding
        // past the node bounds. Drawn after the plot so they sit on
        // top of the line.
        if show_chrome && !points.is_empty() {
            let painter = ctx.ui.painter();
            let (mut tmax, mut vmin, mut vmax) =
                (f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
            for p in points.iter() {
                tmax = tmax.max(p[0]);
                vmin = vmin.min(p[1]);
                vmax = vmax.max(p[1]);
            }
            // Faint horizontal grid (3 lines) + one vertical mid-line.
            // Derived from theme.colors.surface2 at low alpha so the
            // grid stays subtle on whichever card fill is active.
            let grid_color = theme.colors.surface2.alpha(60);
            let grid_stroke = egui::Stroke::new(1.0, grid_color);
            for f in [0.25, 0.5, 0.75] {
                let y = plot_rect.min.y + plot_rect.height() * f;
                painter.line_segment(
                    [egui::pos2(plot_rect.min.x, y), egui::pos2(plot_rect.max.x, y)],
                    grid_stroke,
                );
            }
            let mid_x = plot_rect.min.x + plot_rect.width() * 0.5;
            painter.line_segment(
                [egui::pos2(mid_x, plot_rect.min.y), egui::pos2(mid_x, plot_rect.max.y)],
                grid_stroke,
            );

            // Corner labels use the signal's short name on the Y
            // axis ("F: 26.0k", not just "26.0k") and an explicit
            // `t` on the X axis. Theme-driven colour so they stay
            // readable on any card fill.
            let label_color = theme.tokens.text;
            let font = egui::FontId::monospace(9.0);
            let pad = 3.0;
            let var = short_name(&self.data.signal_path);
            // y-axis: max top-left, min bottom-left
            painter.text(
                egui::pos2(plot_rect.min.x + pad, plot_rect.min.y + pad),
                egui::Align2::LEFT_TOP,
                format!("{var}: {}", format_axis_value(vmax)),
                font.clone(),
                label_color,
            );
            painter.text(
                egui::pos2(plot_rect.min.x + pad, plot_rect.max.y - pad),
                egui::Align2::LEFT_BOTTOM,
                format!("{var}: {}", format_axis_value(vmin)),
                font.clone(),
                label_color,
            );
            // x-axis: tmax bottom-right; tmin omitted (always 0 for
            // a fresh sim, and putting two labels on the same row
            // crowds the corner — "t: <max> s" is enough to give a
            // reader the time scale).
            painter.text(
                egui::pos2(plot_rect.max.x - pad, plot_rect.max.y - pad),
                egui::Align2::RIGHT_BOTTOM,
                format!("t: {} s", format_axis_value(tmax)),
                font,
                label_color,
            );
        }
    }

    fn debug_name(&self) -> &str {
        PLOT_NODE_KIND
    }
}

/// Compact, fixed-width formatter for in-plot axis labels. Engineering
/// suffixes (n/μ/m/k/M/G/T) keep mid-range values readable, falling
/// back to scientific notation outside that range so a numerical
/// blow-up renders as `1e+87` instead of an unreadable string of
/// digits that escapes the corner.
fn format_axis_value(v: f64) -> String {
    if !v.is_finite() {
        return "—".to_string();
    }
    if v == 0.0 {
        return "0".to_string();
    }
    let av = v.abs();
    // Out of engineering range — fall back to compact scientific.
    if av >= 1.0e15 || av < 1.0e-9 {
        return format!("{:.1e}", v);
    }
    let (scale, suffix) = if av >= 1.0e12 {
        (1.0e12, "T")
    } else if av >= 1.0e9 {
        (1.0e9, "G")
    } else if av >= 1.0e6 {
        (1.0e6, "M")
    } else if av >= 1.0e3 {
        (1.0e3, "k")
    } else if av >= 1.0 {
        (1.0, "")
    } else if av >= 1.0e-3 {
        (1.0e-3, "m")
    } else if av >= 1.0e-6 {
        (1.0e-6, "μ")
    } else {
        (1.0e-9, "n")
    };
    let scaled = v / scale;
    if scaled.abs() >= 100.0 {
        format!("{:.0}{}", scaled, suffix)
    } else if scaled.abs() >= 10.0 {
        format!("{:.1}{}", scaled, suffix)
    } else {
        format!("{:.2}{}", scaled, suffix)
    }
}

/// Last dotted segment of a Modelica path — `nozzle.F` → `F`,
/// `body.m_total` → `m_total`. Used as a short axis-label prefix
/// inside the plot corner overlays.
fn short_name(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

/// Convenience: register this kind with a `VisualRegistry`.
/// Domain crates that wire `lunco-canvas` + `lunco-viz` call this
/// once at plugin-build time.
pub fn register(reg: &mut lunco_canvas::VisualRegistry) {
    reg.register_node_kind(PLOT_NODE_KIND, |data: &lunco_canvas::NodeData| {
        // Downcast to the typed payload boxed by callers (e.g.
        // lunco-modelica's plot creator). Empty/wrong-type → render
        // a default plot stub.
        let payload = data
            .downcast_ref::<PlotNodeData>()
            .cloned()
            .unwrap_or_default();
        PlotNodeVisual::from_data(payload)
    });
}
