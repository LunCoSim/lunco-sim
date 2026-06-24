//! Edge / wire types and orthogonal routing for the canvas diagram.
//!
//! Houses [`PortDir`] (cardinal-edge classification used by the
//! routing pass), [`OrthogonalEdgeVisual`] (the per-edge `EdgeVisual`
//! that actually paints the wire — Z-bend / L-elbow / authored
//! waypoints, plus flow animation + hover tooltip), the
//! [`route_orthogonal`] geometry routine, [`ConnectionEdgeData`]
//! (typed payload for `"modelica.connection"` edges in the canvas
//! scene), and the [`edge_hover_text`] tooltip builder.

use bevy_egui::egui;
use lunco_canvas::{DrawCtx, EdgeVisual, Pos as CanvasPos};

use super::paint::{
    brighten, dist_point_to_segment, paint_arrowhead, paint_wire_tooltip, segment_dist_sq,
    wire_color_for,
};
use super::node::paint_flow_dots;
use super::theme::modelica_icon_palette_from_ctx;

/// Typed payload for `"modelica.connection"` edges. Same purpose as
/// [`IconNodeData`](super::IconNodeData).
#[derive(Clone, Debug, Default)]
pub struct ConnectionEdgeData {
    pub connector_type: String,
    pub from_dir: PortDir,
    pub to_dir: PortDir,
    pub icon_color: Option<egui::Color32>,
    pub source_path: String,
    pub target_path: String,
    pub kind: crate::visual_diagram::PortKind,
    pub flow_vars: Vec<crate::visual_diagram::FlowVarMeta>,
    /// True when the source `Line` annotation carried
    /// `smooth=Smooth.Bezier`. Renderer switches from straight
    /// orthogonal segments to a Catmull-Rom-style curve through the
    /// polyline points.
    pub smooth_bezier: bool,
    /// `Line(thickness=…)` override from source. `1.0` = default; the
    /// renderer multiplies its base stroke width by this. The
    /// Modelica default 0.25 round-trips as `1.0` here (it's the
    /// "leave alone" sentinel).
    pub thickness_scale: f32,
}

/// Which edge of the icon a port sits on. Determines which axis the
/// wire's first segment ("stub") runs along — Dymola/OMEdit wire
/// pretty-routing convention. Modelica port placement is in (-100..100)
/// per axis; we classify by which extreme the port sits closest to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PortDir {
    Left,
    Right,
    Up,
    Down,
    /// Port sits in the interior of the icon (or no info). Routing
    /// degrades to plain Z-bend.
    #[default]
    None,
}

impl PortDir {
    /// Unit vector pointing *outward* from the icon at this edge,
    /// in screen coordinates (+Y down). Used to extend the wire
    /// stub away from the icon body.
    pub(super) fn outward(self) -> (f32, f32) {
        match self {
            PortDir::Left => (-1.0, 0.0),
            PortDir::Right => (1.0, 0.0),
            PortDir::Up => (0.0, -1.0),
            PortDir::Down => (0.0, 1.0),
            PortDir::None => (0.0, 0.0),
        }
    }
}

/// Classify a 2D direction into one of the four cardinal icon edges,
/// in **screen frame** (+X right, +Y down — same convention as
/// [`PortDir::outward`]). Used to decide which way a wire stub
/// should extend out of a port.
///
/// The threshold makes any direction whose components are both close
/// to zero collapse to [`PortDir::None`] — Z-bend routing falls
/// through to the original midpoint logic in that case.
pub(super) fn port_edge_dir(x: f32, y: f32) -> PortDir {
    let threshold = 0.4;
    let ax = x.abs();
    let ay = y.abs();
    if ax < threshold && ay < threshold {
        return PortDir::None;
    }
    if ax >= ay {
        if x >= 0.0 { PortDir::Right } else { PortDir::Left }
    } else if y >= 0.0 {
        PortDir::Down
    } else {
        PortDir::Up
    }
}

/// Per-edge wire visual. Carries the wire colour + the port-direction
/// hints baked in by the projector so each edge knows which axis to
/// extend before bending.
pub(super) struct OrthogonalEdgeVisual {
    pub(super) color: egui::Color32,
    pub(super) from_dir: PortDir,
    pub(super) to_dir: PortDir,
    pub(super) is_causal: bool,
    pub(super) source_path: String,
    pub(super) target_path: String,
    pub(super) flow_vars: Vec<crate::visual_diagram::FlowVarMeta>,
    pub(super) connector_leaf: String,
    /// Pre-built `("source.fv", "target.fv")` keys for the first
    /// flow var, materialised at projection time so per-frame `draw`
    /// does zero allocation on the lookup path. `None` when the edge
    /// has no flow vars.
    pub(super) flow_lookup_keys: Option<(String, String)>,
    /// Render the wire as a smooth curve when set — mirrors the
    /// `smooth=Smooth.Bezier` source annotation.
    pub(super) smooth_bezier: bool,
    /// Multiplier on the base stroke width; 1.0 = default. Derived
    /// from `Line(thickness=…)` in source.
    pub(super) thickness_scale: f32,
}

impl Default for OrthogonalEdgeVisual {
    fn default() -> Self {
        Self {
            color: wire_color_for(""),
            from_dir: PortDir::None,
            to_dir: PortDir::None,
            is_causal: false,
            source_path: String::new(),
            target_path: String::new(),
            flow_vars: Vec::new(),
            connector_leaf: String::new(),
            flow_lookup_keys: None,
            smooth_bezier: false,
            thickness_scale: 1.0,
        }
    }
}

const STUB_PX: f32 = 18.0;

impl EdgeVisual for OrthogonalEdgeVisual {
    fn draw(
        &self,
        ctx: &mut DrawCtx,
        from: CanvasPos,
        to: CanvasPos,
        waypoints_screen: &[CanvasPos],
        selected: bool,
    ) {
        let palette = modelica_icon_palette_from_ctx(ctx.ui.ctx());
        let mapped = palette
            .as_ref()
            .map(|p| p.remap(self.color))
            .unwrap_or(self.color);
        let col = if selected {
            brighten(mapped)
        } else {
            mapped
        };
        let base_width = if selected {
            if self.is_causal { 2.2 } else { 1.7 }
        } else if self.is_causal {
            1.6
        } else {
            1.1
        };
        let zoom_norm = (ctx.viewport.zoom / 3.0).sqrt().clamp(0.7, 1.6);
        let _w0 = base_width * zoom_norm;
        let scale = zoom_norm;
        let width = base_width * scale * self.thickness_scale.max(0.1);
        let stroke = egui::Stroke::new(width, col);
        let painter = ctx.ui.painter();

        // Resolve the wire's polyline (authored waypoints when sane,
        // otherwise the orthogonal router). We bind it to a single
        // `polyline` variable so the API-pulse overlay below can
        // reuse the *same* path the wire was just drawn on, instead
        // of inventing a straight from→to line that ignores stubs
        // and waypoints.
        let polyline: Vec<egui::Pos2> = 'route: {
            // The caller passes the live waypoints from `Edge::waypoints`
            // (mutated by the canvas tool during a drag), so the wire
            // follows the cursor without waiting for a re-projection.
            let from_screen = egui::pos2(from.x, from.y);
            let to_screen = egui::pos2(to.x, to.y);
            let way_screen: Vec<egui::Pos2> = waypoints_screen
                .iter()
                .map(|p| egui::pos2(p.x, p.y))
                .collect();
            if !way_screen.is_empty() {
                // Trust authored waypoints. The polyline is always
                // from-port → waypoints → to-port; the first/last
                // segment is drawn straight even if it's diagonal
                // (Dymola convention — users may bend a wire off the
                // port axis on purpose). Earlier code rejected non-
                // axis-aligned endpoints and fell back to the auto-
                // router, which silently discarded user edits.
                let mut pts = Vec::with_capacity(way_screen.len() + 2);
                pts.push(from_screen);
                pts.extend(way_screen.iter().copied());
                pts.push(to_screen);
                break 'route pts;
            }
            route_orthogonal(
                egui::pos2(from.x, from.y),
                self.from_dir,
                egui::pos2(to.x, to.y),
                self.to_dir,
                STUB_PX * scale,
            )
        };
        if self.smooth_bezier && polyline.len() >= 3 {
            // Catmull-Rom-style smoothing: sample a cubic spline
            // through the polyline points and emit a path of short
            // line segments approximating the curve. Cheap enough at
            // typical wire complexity (≤ 8 segments × 16 samples).
            let smoothed = sample_catmull_rom(&polyline, 16);
            for w in smoothed.windows(2) {
                painter.line_segment([w[0], w[1]], stroke);
            }
        } else {
            for w in polyline.windows(2) {
                painter.line_segment([w[0], w[1]], stroke);
            }
        }

        // ── API edge-add pulse overlay ──────────────────────────────
        // Read the live alpha map published by `EdgePulseLayer` and
        // overlay a warm yellow-orange highlight along the *same*
        // polyline at the same zoom scale, so the flash follows the
        // wire's stubs/waypoints and tracks zoom instead of being a
        // straight, fixed-pixel-width screen-space line.
        let pulse_alpha: Option<f32> = {
            let id = egui::Id::new(
                super::pulse::EDGE_PULSE_DATA_ID,
            );
            ctx.ui.ctx().data(|d| {
                d.get_temp::<super::pulse::EdgePulseAlphaMap>(id)
                    .and_then(|m| {
                        m.get(&(self.source_path.clone(), self.target_path.clone()))
                            .or_else(|| {
                                m.get(&(self.target_path.clone(), self.source_path.clone()))
                            })
                            .copied()
                    })
            })
        };
        if let Some(alpha) = pulse_alpha {
            let base = egui::Color32::from_rgb(255, 196, 60);
            let halo_w = (base_width * scale * 4.5).max(2.0);
            let line_w = (base_width * scale * 1.6).max(1.0);
            let halo_a = (alpha * 0.55 * 255.0).clamp(0.0, 255.0) as u8;
            let line_a = (alpha * 0.95 * 255.0).clamp(0.0, 255.0) as u8;
            let halo = egui::Stroke::new(
                halo_w,
                egui::Color32::from_rgba_unmultiplied(
                    base.r(), base.g(), base.b(), halo_a,
                ),
            );
            let core = egui::Stroke::new(
                line_w,
                egui::Color32::from_rgba_unmultiplied(
                    base.r(), base.g(), base.b(), line_a,
                ),
            );
            for w in polyline.windows(2) {
                painter.line_segment([w[0], w[1]], halo);
            }
            for w in polyline.windows(2) {
                painter.line_segment([w[0], w[1]], core);
            }
        }

        if false && self.is_causal && polyline.len() >= 2 {
            let n = polyline.len();
            paint_arrowhead(
                painter,
                polyline[n - 2],
                polyline[n - 1],
                col,
                scale,
            );
        }

        let anim_time = ctx
            .ui
            .ctx()
            .data(|d| {
                d.get_temp::<f64>(egui::Id::new("lunco_modelica_flow_anim_time"))
            })
            .unwrap_or(0.0);
        let node_state =
            lunco_viz::kinds::canvas_plot_node::fetch_node_state(ctx.ui.ctx());
        const ACTIVITY_EPS: f64 = 1e-6;
        let physical_flow = if let (Some(fv), Some((src_key, tgt_key))) =
            (self.flow_vars.first(), self.flow_lookup_keys.as_ref())
        {
            let v_src = node_state.values.get(src_key.as_str()).copied();
            let v_tgt = node_state.values.get(tgt_key.as_str()).copied();
            // ── DIAG: log once per (source_path,target_path,fv) which
            // keys hit/missed, and what near-miss keys exist in
            // node_state. Helps diagnose why some edges animate only
            // after a re-projection. Remove once root cause found.
            diag_log_edge_lookup(
                &self.source_path,
                &self.target_path,
                &fv.name,
                v_src,
                v_tgt,
                &node_state,
            );
            if let Some(v) = v_src {
                Some(-v)
            } else {
                v_tgt
            }
        } else {
            // ── DIAG: log once per (source_path,target_path) that
            // this edge has empty flow_vars (so the projection
            // didn't resolve the connector's flow declarations).
            diag_log_empty_flow_vars(
                &self.source_path,
                &self.target_path,
                &self.connector_leaf,
                &node_state,
            );
            node_state
                .values
                .get(&self.source_path)
                .or_else(|| node_state.values.get(&self.target_path))
                .map(|&v| v.abs())
        };
        if let Some(v) = physical_flow {
            if v.abs() > ACTIVITY_EPS {
                if v < 0.0 {
                    let mut rev = polyline.clone();
                    rev.reverse();
                    paint_flow_dots(painter, &rev, col, anim_time, scale);
                } else {
                    paint_flow_dots(painter, &polyline, col, anim_time, scale);
                }
            }
        }

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

        // ── Editable waypoint handles ────────────────────────────────────
        // Dymola/OMEdit: when a wire is selected, paint a filled square
        // at every interior corner and an outlined square at the
        // midpoint of every reasonably-long segment. The canvas tool
        // hit-tests these geometrically against `Edge::waypoints`, so
        // the paint here only needs to mirror that hit geometry.
        if selected {
            let handle_size = (4.0 * scale).max(3.0);
            let fill = egui::Color32::from_rgb(255, 196, 60);
            let outline = egui::Stroke::new(1.0, egui::Color32::BLACK);
            let hover_pos = ctx.ui.ctx().pointer_hover_pos();
            let mut cursor: Option<egui::CursorIcon> = None;
            // Corner handles — one per interior waypoint.
            for w in waypoints_screen {
                let r = egui::Rect::from_center_size(
                    egui::pos2(w.x, w.y),
                    egui::vec2(handle_size, handle_size),
                );
                painter.rect_filled(r, 0.0, fill);
                painter.rect_stroke(r, 0.0, outline, egui::StrokeKind::Outside);
                if let Some(p) = hover_pos {
                    if r.expand(2.0).contains(p) {
                        cursor = Some(egui::CursorIcon::Move);
                    }
                }
            }
            // Segment midpoint handles — outlined squares; skip
            // segments shorter than ~3× the handle so they don't
            // overlap corner handles or each other. Hover sets a
            // resize-N/S or resize-E/W cursor depending on the
            // segment's dominant axis (perpendicular slide).
            let min_seg = handle_size * 3.0;
            for w in polyline.windows(2) {
                let a = w[0];
                let b = w[1];
                let dx = b.x - a.x;
                let dy = b.y - a.y;
                let len = (dx * dx + dy * dy).sqrt();
                if len < min_seg {
                    continue;
                }
                let mid = egui::pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
                let r = egui::Rect::from_center_size(
                    mid,
                    egui::vec2(handle_size, handle_size),
                );
                painter.rect_stroke(
                    r,
                    0.0,
                    egui::Stroke::new(1.2, fill),
                    egui::StrokeKind::Outside,
                );
                if let Some(p) = hover_pos {
                    if r.expand(2.0).contains(p) && cursor.is_none() {
                        cursor = Some(if dx.abs() >= dy.abs() {
                            egui::CursorIcon::ResizeVertical
                        } else {
                            egui::CursorIcon::ResizeHorizontal
                        });
                    }
                }
            }
            if let Some(c) = cursor {
                ctx.ui.ctx().set_cursor_icon(c);
            }
        }
    }

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

/// Compute an orthogonal polyline routed between two ports, in
/// **screen coords** (+Y down). The router emits a stub from each
/// port in its outward direction, then connects the stub-ends with
/// either an L-elbow (perpendicular ports) or a Z-bend (parallel /
/// unknown), choosing pivot positions that keep the wire from
/// doubling back across the icon body.
pub(super) fn route_orthogonal(
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

    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let from_helps = uxf * dx + uyf * dy > 0.0;
    let to_helps = uxt * (-dx) + uyt * (-dy) > 0.0;

    let mut pts: Vec<egui::Pos2> = Vec::with_capacity(6);
    pts.push(from);

    if (f_horiz && t_vert) || (f_vert && t_horiz) {
        let corner = if f_horiz {
            egui::pos2(to.x, from.y)
        } else {
            egui::pos2(from.x, to.y)
        };
        if corner != from && corner != to {
            pts.push(corner);
        }
        pts.push(to);
        pts.dedup_by(|a, b| (a.x - b.x).abs() < 0.5 && (a.y - b.y).abs() < 0.5);
        return pts;
    }
    if from_dir != None {
        pts.push(f_stub);
    }

    if f_horiz && t_horiz {
        let pivot_x = if from_helps && to_helps {
            (f_stub.x + t_stub.x) * 0.5
        } else if !from_helps {
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

    pts.dedup_by(|a, b| (a.x - b.x).abs() < 0.5 && (a.y - b.y).abs() < 0.5);
    pts
}

/// Sample a Catmull-Rom spline through the control points, emitting
/// `samples_per_segment` straight line segments per pair of control
/// points so egui's stroke pipeline can render the curve. Endpoint
/// handling uses the doubled-endpoint trick (P[-1] = P[0], P[N] =
/// P[N-1]) so the curve starts and ends exactly at the port stubs.
fn sample_catmull_rom(pts: &[egui::Pos2], samples_per_segment: usize) -> Vec<egui::Pos2> {
    if pts.len() < 2 {
        return pts.to_vec();
    }
    let n = pts.len();
    let mut out: Vec<egui::Pos2> = Vec::with_capacity((n - 1) * samples_per_segment + 1);
    out.push(pts[0]);
    for i in 0..n - 1 {
        let p0 = if i == 0 { pts[0] } else { pts[i - 1] };
        let p1 = pts[i];
        let p2 = pts[i + 1];
        let p3 = if i + 2 < n { pts[i + 2] } else { pts[i + 1] };
        for s in 1..=samples_per_segment {
            let t = s as f32 / samples_per_segment as f32;
            let t2 = t * t;
            let t3 = t2 * t;
            // Standard Catmull-Rom basis with tension 0.5.
            let x = 0.5
                * ((2.0 * p1.x)
                    + (-p0.x + p2.x) * t
                    + (2.0 * p0.x - 5.0 * p1.x + 4.0 * p2.x - p3.x) * t2
                    + (-p0.x + 3.0 * p1.x - 3.0 * p2.x + p3.x) * t3);
            let y = 0.5
                * ((2.0 * p1.y)
                    + (-p0.y + p2.y) * t
                    + (2.0 * p0.y - 5.0 * p1.y + 4.0 * p2.y - p3.y) * t2
                    + (-p0.y + 3.0 * p1.y - 3.0 * p2.y + p3.y) * t3);
            out.push(egui::pos2(x, y));
        }
    }
    out
}

/// Build the wire hover tooltip text from AST-derived semantics —
/// header = connector class short-name; one line per declared flow
/// variable (name = value unit) for acausal connectors; otherwise
/// one line for the source-port value itself (causal signals).
/// Formats "n/a" for variables the sim hasn't sampled yet.
pub(super) fn edge_hover_text(
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

// ── Diagnostics for the flow-animation lookup ──────────────────────
//
// One-shot per (source_path,target_path,key) so the log doesn't drown
// the console at 60 fps. Drop once the root cause of "tank↔valve only
// animates after I move a node" is identified.

use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiagStatus {
    BothMiss,
    SrcHit,
    TgtHit,
    Both,
}

thread_local! {
    /// Last logged status per edge lookup; we re-log only on state
    /// transitions, so the console captures exactly when a key
    /// becomes available (or disappears) instead of one snapshot at
    /// startup.
    static DIAG_LOOKUP_STATE: RefCell<HashMap<String, DiagStatus>> =
        RefCell::new(HashMap::new());
    /// Last logged total snapshot-key count per (source,target) edge
    /// when flow_vars is empty — log when "near_keys" set
    /// changes size (a proxy for the simulator publishing new
    /// connector vars).
    static DIAG_EMPTY_STATE: RefCell<HashMap<String, usize>> =
        RefCell::new(HashMap::new());
}

fn diag_log_edge_lookup(
    source_path: &str,
    target_path: &str,
    fv_name: &str,
    v_src: Option<f64>,
    v_tgt: Option<f64>,
    state: &lunco_viz::kinds::canvas_plot_node::NodeStateSnapshot,
) {
    let status = match (v_src.is_some(), v_tgt.is_some()) {
        (false, false) => DiagStatus::BothMiss,
        (true, false) => DiagStatus::SrcHit,
        (false, true) => DiagStatus::TgtHit,
        (true, true) => DiagStatus::Both,
    };
    let key = format!("{source_path}|{target_path}|{fv_name}");
    let changed = DIAG_LOOKUP_STATE.with(|s| {
        let mut m = s.borrow_mut();
        match m.get(&key) {
            Some(prev) if *prev == status => false,
            _ => {
                m.insert(key.clone(), status);
                true
            }
        }
    });
    if !changed {
        return;
    }
    let near: Vec<String> = state
        .values
        .keys()
        .filter(|k| k.starts_with(source_path) || k.starts_with(target_path))
        .cloned()
        .collect();
    bevy::log::debug!(
        "[edge-diag] {source_path} -> {target_path} fv={fv_name} \
         src={v_src:?} tgt={v_tgt:?} near_keys={near:?}"
    );
}

fn diag_log_empty_flow_vars(
    source_path: &str,
    target_path: &str,
    connector_type: &str,
    state: &lunco_viz::kinds::canvas_plot_node::NodeStateSnapshot,
) {
    let near: Vec<String> = state
        .values
        .keys()
        .filter(|k| k.starts_with(source_path) || k.starts_with(target_path))
        .cloned()
        .collect();
    let key = format!("{source_path}|{target_path}");
    let changed = DIAG_EMPTY_STATE.with(|s| {
        let mut m = s.borrow_mut();
        match m.get(&key) {
            Some(prev) if *prev == near.len() => false,
            _ => {
                m.insert(key.clone(), near.len());
                true
            }
        }
    });
    if !changed {
        return;
    }
    bevy::log::debug!(
        "[edge-diag] EMPTY flow_vars on edge {source_path} -> {target_path} \
         connector_type={connector_type:?} near_keys={near:?}"
    );
}
