//! API-driven focus + connection pulse layers.
//!
//! Edges connecting newly-API-added components flash for a short
//! window so users notice them; recently-API-focused entities glow
//! with an outer ring. Both effects are built from per-entry
//! `PulseEntry<T>` records driven by background tickers
//! (`drive_pending_api_focus`, `drive_pending_api_connections`).

use bevy::prelude::*;

use super::CanvasDiagramState;
// The API-feedback queue *data* lives in the egui-free core module
// `crate::canvas_feedback`; these UI systems drain it.
use crate::canvas_feedback::{PendingApiConnectionQueue, PendingApiFocusQueue};

/// Window for batch-collapse: if a new entry arrives within this of
/// the previous one, the system holds back from focusing on the older
/// entries individually and instead waits for the burst to end.
const BATCH_WINDOW: std::time::Duration = std::time::Duration::from_millis(200);

/// Hard timeout — drop a queued focus if no node with the given origin
/// has appeared in the scene by then. Stops the queue from leaking on
/// failed AddComponent ops or rename races.
const FOCUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Stagger between consecutive node-pulse start times within a batch.
/// Adds a "slight delay between elements" feel (per user feedback)
/// without actually delaying the source mutation — the components
/// land in the scene at once; the *pulse* is what reveals them in
/// sequence. Empty for batch=1.
const PULSE_STAGGER_MS: u64 = 250;

/// Edge-pulse coordinator layer. Drawing happens inside
/// `OrthogonalEdgeVisual::draw` so the highlight follows the wire's
/// actual routed polyline (orthogonal stubs + waypoints) and tracks
/// the canvas zoom, instead of being a straight, fixed-pixel-width
/// stroke from port to port. This layer prunes expired entries and
/// publishes the live `(from_path, to_path) → alpha` map into egui
/// ctx data under [`EDGE_PULSE_DATA_ID`] for the visual to consume.
pub(super) struct EdgePulseLayer {
    pub(super) data: EdgePulseHandle,
}

/// `egui::Id` under which the per-edge pulse alphas are published for
/// `OrthogonalEdgeVisual::draw` to read. The stored value is a
/// `HashMap<(String,String), f32>` keyed by the wire's
/// `(source_path, target_path)` paths (e.g. `"solar_pulse.y" →
/// "solar_in.Q_flow"`).
pub(crate) const EDGE_PULSE_DATA_ID: &str = "lunco_modelica_edge_pulse_alphas";

/// Live pulse map type stashed in egui ctx data per frame.
pub(crate) type EdgePulseAlphaMap = std::collections::HashMap<(String, String), f32>;

impl lunco_canvas::Layer for EdgePulseLayer {
    fn name(&self) -> &'static str {
        "modelica.edge_pulse"
    }

    fn draw(
        &mut self,
        ctx: &mut lunco_canvas::visual::DrawCtx,
        _scene: &lunco_canvas::Scene,
        _selection: &lunco_canvas::Selection,
    ) {
        let alphas: EdgePulseAlphaMap = {
            let Ok(mut guard) = self.data.write() else {
                return;
            };
            let now = web_time::Instant::now();
            guard.retain(|e| match now.checked_duration_since(e.started) {
                Some(d) => d.as_millis() < e.duration_ms as u128,
                None => true,
            });
            guard
                .iter()
                .filter_map(|e| {
                    let alpha = match now.checked_duration_since(e.started) {
                        None => 0.0,
                        Some(elapsed) => {
                            let age_ms = elapsed.as_secs_f32() * 1000.0;
                            let total_ms = (e.duration_ms as f32).max(1.0);
                            let t = (age_ms / total_ms).clamp(0.0, 1.0);
                            1.0 - t.powi(4)
                        }
                    };
                    if alpha > 0.001 {
                        Some(((e.from_path.clone(), e.to_path.clone()), alpha))
                    } else {
                        None
                    }
                })
                .collect()
        };
        // Always publish (even when empty) so a stale value from the
        // previous frame doesn't keep ghost-flashing forever.
        ctx.ui.ctx().data_mut(|d| {
            d.insert_temp(bevy_egui::egui::Id::new(EDGE_PULSE_DATA_ID), alphas);
        });
        // Repaint while pulses are alive so the decay curve advances.
        if self.data.read().map(|g| !g.is_empty()).unwrap_or(false) {
            ctx.ui.ctx().request_repaint();
        }
    }
}

/// Per-frame driver for connection adds. The match-by-edge-id dance
/// is gone: pulses are now keyed by the wire's `(from_path, to_path)`
/// dot-form, so the per-edge visual matches itself when it draws.
/// We just push one record per (doc, edge) into every tab viewing
/// the doc. `OrthogonalEdgeVisual::draw` reads the live alpha map
/// published by [`EdgePulseLayer`] and overlays the highlight along
/// its already-routed polyline.
pub fn drive_pending_api_connections(
    mut queue: ResMut<PendingApiConnectionQueue>,
    mut state: ResMut<CanvasDiagramState>,
) {
    if queue.0.is_empty() {
        return;
    }
    let now = web_time::Instant::now();
    for entry in queue.0.drain(..) {
        if now.duration_since(entry.queued_at) > FOCUS_TIMEOUT {
            continue;
        }
        let anim_ms = entry.animation_ms;
        if anim_ms == 0 {
            continue;
        }
        let from_path = format!("{}.{}", entry.from_component, entry.from_port);
        let to_path = format!("{}.{}", entry.to_component, entry.to_port);
        for (_, d, ds) in state.iter_mut() {
            if d != entry.doc {
                continue;
            }
            if let Ok(mut guard) = ds.edge_pulse_handle.write() {
                guard.push(EdgePulseRecord {
                    from_path: from_path.clone(),
                    to_path: to_path.clone(),
                    started: web_time::Instant::now(),
                    duration_ms: anim_ms,
                });
            }
        }
    }
}

// ─── Cinematic camera ──────────────────────────────────────────────────
//
// Replaces `viewport.set_target`'s constant exponential smoothing with
// a keyframe-driven curve. Lets us do shot types — pure dolly, focus
// pull (zoom-out + hold + zoom-in), establishing shot — instead of
// always linearly easing toward the target. Frame-rate independent;
// driven by elapsed wall-clock.
//
// Why a keyframe model: a single `Tween { from, to, duration, ease }`
// can't express the "pull back, hold, push in" shape that makes
// distant targets feel intentional rather than swoopy. Keyframes are
// the standard movie-camera abstraction: anchor a curve at each
// time offset, blend in between.
//
// While a cinematic is active, the viewport's built-in tween must not
// also drift the values, so each frame we snap-set both current AND
// target to the eased keyframe value (`viewport.snap_to`).

/// One pulse-glow entry: target id, when it started, and how long it
/// should last (per-call duration; the API caller can pass
/// `animation_ms` on the command to override the default). Splitting
/// per-entry instead of using a single global constant lets callers
/// mix instant adds with cinematic ones.
#[derive(Debug, Clone, Copy)]
pub struct PulseEntry<T> {
    pub id: T,
    pub started: web_time::Instant,
    pub duration_ms: u32,
}

/// Per-doc node-pulse registry. Vec rather than HashMap because we
/// expect ≤ a few entries at a time and iteration order doesn't
/// matter — the layer re-walks every frame anyway.
pub type PulseHandle = std::sync::Arc<std::sync::RwLock<Vec<PulseEntry<lunco_canvas::NodeId>>>>;

/// One live edge-pulse, keyed by the wire's `(from_path, to_path)`
/// dot-form so the per-edge visual can match without a scene-edge id
/// (whose value depends on projection ordering and is invalidated by
/// every reproject).
#[derive(Debug, Clone)]
pub struct EdgePulseRecord {
    pub from_path: String,
    pub to_path: String,
    pub started: web_time::Instant,
    pub duration_ms: u32,
}

/// Edge-pulse registry. Pushed into by
/// [`drive_pending_api_connections`] when a `ConnectComponents` call
/// lands; drained-by-decay by [`EdgePulseLayer`].
pub type EdgePulseHandle = std::sync::Arc<std::sync::RwLock<Vec<EdgePulseRecord>>>;

/// Outer-glow render layer: paints a soft ring around each
/// recently-added node, alpha decaying linearly to 0 over
/// `PULSE_DURATION`. Figma-style — see `docs/architecture/20-domain-modelica.md`
/// § 9c.4 for the design rationale.
pub(super) struct PulseGlowLayer {
    pub(super) data: PulseHandle,
}

impl lunco_canvas::Layer for PulseGlowLayer {
    fn name(&self) -> &'static str {
        "modelica.pulse_glow"
    }

    fn draw(
        &mut self,
        ctx: &mut lunco_canvas::visual::DrawCtx,
        scene: &lunco_canvas::Scene,
        _selection: &lunco_canvas::Selection,
    ) {
        // First, walk + decay; collect (node_id, alpha) for entries
        // still alive. Drop the write guard before any heavy painting.
        let live: Vec<(lunco_canvas::NodeId, f32)> = {
            let Ok(mut guard) = self.data.write() else {
                return;
            };
            let now = web_time::Instant::now();
            // Drop entries whose start+duration has elapsed. Entries
            // whose `started` is still in the future stay (they were
            // staggered by the focus driver — see PULSE_STAGGER_MS).
            // Per-entry duration: each call carries its own
            // `duration_ms` so a caller can pass `animation_ms = 500`
            // for a quick add or `animation_ms = 0` to skip the
            // glow.
            guard.retain(|e| match now.checked_duration_since(e.started) {
                Some(d) => d.as_millis() < e.duration_ms as u128,
                None => true,
            });
            guard
                .iter()
                .map(|e| {
                    let alpha = match now.checked_duration_since(e.started) {
                        None => 0.0,
                        Some(elapsed) => {
                            let age_ms = elapsed.as_secs_f32() * 1000.0;
                            let total_ms = (e.duration_ms as f32).max(1.0);
                            let t = (age_ms / total_ms).clamp(0.0, 1.0);
                            1.0 - t.powi(4)
                        }
                    };
                    (e.id, alpha)
                })
                .filter(|(_, a)| *a > 0.001)
                .collect()
        };
        if live.is_empty() {
            return;
        }
        let painter = ctx.ui.painter();
        let theme = lunco_theme::active(ctx.ui.ctx());
        // Use the theme's accent color as the glow base — ties
        // visually to the rest of the canvas chrome and shifts with
        // the active theme. Multiplied by per-entry alpha and a
        // global pulse intensity (0.65) so the glow stays subtle.
        let base = theme.tokens.accent;
        for (node_id, alpha) in live {
            let Some(node) = scene.node(node_id) else {
                continue;
            };
            let world_rect = node.rect;
            let screen = ctx
                .viewport
                .world_rect_to_screen(world_rect, ctx.screen_rect);
            let r = bevy_egui::egui::Rect::from_min_max(
                bevy_egui::egui::pos2(screen.min.x, screen.min.y),
                bevy_egui::egui::pos2(screen.max.x, screen.max.y),
            );
            // Stack 4 expanding outlines with decreasing alpha — the
            // cheapest convincing outer-glow you can do with egui's
            // stroke API. Each layer doubles its outset and halves its
            // opacity, producing a soft falloff.
            for ring in 0..4 {
                let outset = (ring as f32 + 1.0) * 3.0;
                let ring_rect = r.expand(outset);
                let ring_alpha = alpha * 0.65 * (1.0 - ring as f32 * 0.22);
                let a = (ring_alpha * 255.0).clamp(0.0, 255.0) as u8;
                let color = bevy_egui::egui::Color32::from_rgba_unmultiplied(
                    base.r(),
                    base.g(),
                    base.b(),
                    a,
                );
                painter.rect_stroke(
                    ring_rect,
                    bevy_egui::egui::CornerRadius::same(2),
                    bevy_egui::egui::Stroke::new(2.0, color),
                    bevy_egui::egui::StrokeKind::Outside,
                );
            }
        }
    }
}

/// Per-frame driver: drain the focus queue once a *complete* batch has
/// landed in the projected scene, then act ONCE for the whole batch.
/// Designed to avoid the "camera jumps between nodes" feel when N
/// AddComponents arrive across several frames with staggered
/// projection latency.
///
/// Sequence:
///   1. Hold the queue until the latest push is `BATCH_WINDOW` idle.
///   2. Try to match every queued entry. If any is unmatched and not
///      timed out, defer one more frame — keeps the batch atomic.
///   3. Once all matched (or timed out): drain, pulse all, decide the
///      camera move:
///        a. New nodes already inside the viewport → no camera move
///           (Figma/Miro convention — pulse alone signals the change).
///        b. Otherwise → smooth FitVisible over the union of (current
///           visible region ∪ new nodes), so context is preserved.
pub fn drive_pending_api_focus(
    mut queue: ResMut<PendingApiFocusQueue>,
    mut state: ResMut<CanvasDiagramState>,
) {
    if queue.0.is_empty() {
        return;
    }
    let now = web_time::Instant::now();

    // (1) Batch-idle gate.
    if let Some(latest) = queue.0.last() {
        if now.duration_since(latest.queued_at) < BATCH_WINDOW {
            return;
        }
    }

    // (2) Try-match pass — non-draining. Anything unmatched and within
    // FOCUS_TIMEOUT forces us to wait one more frame.
    //
    // We capture entry *names* per-doc rather than pre-resolving
    // `NodeId`s — node ids are scene-local, so a node id from the
    // first tab won't match the same logical node in a sibling
    // tab's scene. The fan-out below re-finds the node per tab.
    let mut matched: std::collections::HashMap<
        lunco_doc::DocumentId,
        Vec<(String /* name */, u32 /* animation_ms */)>,
    > = std::collections::HashMap::new();
    let mut any_still_unmatched_within_timeout = false;
    for entry in queue.0.iter() {
        // Use first-tab projection to test "does this name resolve
        // *somewhere* yet?". The actual per-tab node id is
        // re-resolved in the fan-out.
        let docstate = state.get(Some(entry.doc));
        let resolved = docstate
            .canvas
            .scene
            .nodes()
            .any(|(_, n)| n.origin.as_deref() == Some(entry.name.as_str()));
        if resolved {
            matched
                .entry(entry.doc)
                .or_default()
                .push((entry.name.clone(), entry.animation_ms));
        } else if now.duration_since(entry.queued_at) <= FOCUS_TIMEOUT {
            any_still_unmatched_within_timeout = true;
        }
    }
    if any_still_unmatched_within_timeout {
        return;
    }

    // (3) Whole batch resolved (or timed out). Drain + act.
    queue.0.clear();
    if matched.is_empty() {
        return;
    }

    let now_pulse = web_time::Instant::now();
    // Fan out across every tab viewing each doc. Each tab gets its
    // own pulse entries (resolved by `entry.name` against the tab's
    // scene) and its own `pending_fit` flag — fitting in tab A must
    // not move tab B's camera. Fixes the focus regression where
    // split-view tabs only animated the first tab.
    for (_, d, ds) in state.iter_mut() {
        let Some(entries) = matched.get(&d) else {
            continue;
        };

        if let Ok(mut guard) = ds.pulse_handle.write() {
            for (i, (name, anim_ms)) in entries.iter().enumerate() {
                if *anim_ms == 0 {
                    continue;
                }
                let Some((node_id, _)) = ds
                    .canvas
                    .scene
                    .nodes()
                    .find(|(_, n)| n.origin.as_deref() == Some(name.as_str()))
                else {
                    continue;
                };
                let stagger = std::time::Duration::from_millis(PULSE_STAGGER_MS * i as u64);
                guard.push(PulseEntry {
                    id: *node_id,
                    started: now_pulse + stagger,
                    duration_ms: *anim_ms,
                });
            }
        }

        // Camera move: defer to the canvas render's `pending_fit`
        // branch. That branch runs INSIDE the panel render where the
        // actual `response.rect` is in scope, so the fit math uses
        // the real widget size — not the 1280×800 approximation
        // we'd have to guess at here. It calls
        // `viewport.set_target`, which animates via the viewport's
        // built-in exponential ease.
        ds.pending_fit = true;
    }
}
