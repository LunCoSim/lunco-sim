//! Scope-driven loading indicator widget.
//!
//! Panels build a `LoadingIndicator::for_scope(scope)` and call one of
//! the render flavours (`overlay_on`, `inline`, `banner`). The widget
//! reads [`StatusBus`] to decide whether to paint and what to paint —
//! call sites do not pick visuals, they declare scope.

use std::time::Duration;

use bevy_egui::egui::{self, Align2, FontId, Rect, Vec2};
use lunco_theme::{ColorAlpha, Theme};
use web_time::Instant;

use super::spinner::paint_three_dot;
use super::{BusyScope, StatusBus, StatusEvent};

/// Wait before showing an indicator. Below this elapsed time we paint
/// nothing — fast paths never flicker a spinner. Best-practice band is
/// 100–250 ms; pick the conservative end so a freshly-clicked drill-in
/// has a moment to resolve from cache.
pub(crate) const SHOW_AFTER: Duration = Duration::from_millis(200);

/// Threshold past which the overlay shows an elapsed-time read-out.
/// Below this the user is unlikely to want a precise number; above it
/// they're starting to wonder if the task is wedged.
pub(crate) const ELAPSED_AFTER: Duration = Duration::from_secs(3);

/// Builder for a scope-driven indicator. Call one of the render
/// flavours to paint; if the scope is not busy or has not yet crossed
/// [`SHOW_AFTER`], the call is a no-op.
pub struct LoadingIndicator {
    scope: BusyScope,
}

impl LoadingIndicator {
    /// Indicator that paints when anything within `scope` is busy.
    pub fn for_scope(scope: BusyScope) -> Self {
        Self { scope }
    }

    /// Resolve the entry to render — the longest-running busy entry
    /// in scope whose elapsed time has crossed [`SHOW_AFTER`].
    fn pick<'a>(&self, bus: &'a StatusBus) -> Option<&'a StatusEvent> {
        let ev = bus.longest_in(self.scope)?;
        if Instant::now().saturating_duration_since(ev.at) < SHOW_AFTER {
            return None;
        }
        Some(ev)
    }

    /// Centred card painted over `rect` — the canvas-overlay flavour.
    /// Visually matches the pre-existing canvas drill-in / projection
    /// overlays: spinner left, "{verb}…" header (with elapsed once
    /// past [`ELAPSED_AFTER`]), monospace detail line below.
    /// No-op when the scope is not busy or hasn't crossed [`SHOW_AFTER`].
    pub fn overlay_on(self, ui: &mut egui::Ui, rect: Rect, bus: &StatusBus, theme: &Theme) {
        let Some(ev) = self.pick(bus) else { return };
        // Clip to the host ui's rect intersected with `rect` so a
        // small canvas pane can't paint the card over its neighbour
        // panes.
        let painter = ui
            .painter()
            .clone()
            .with_clip_rect(ui.clip_rect().intersect(rect));

        let card_size = Vec2::new(340.0, 84.0);
        let card_rect = Rect::from_center_size(rect.center(), card_size);

        // Drop shadow tinted from the theme base so it reads on both
        // light and dark themes.
        let shadow = theme.colors.base.alpha(100);
        painter.rect_filled(card_rect.translate(Vec2::new(0.0, 3.0)), 8.0, shadow);
        painter.rect_filled(card_rect, 8.0, theme.tokens.surface_raised);
        painter.rect_stroke(
            card_rect,
            8.0,
            egui::Stroke::new(1.0, theme.tokens.surface_raised_border),
            egui::StrokeKind::Outside,
        );

        // Spinner left-aligned inside the card.
        let dots_centre =
            egui::pos2(card_rect.min.x + 28.0, card_rect.center().y);
        paint_three_dot(ui, dots_centre, theme.tokens.accent);

        // Header verb picked from the bus event's `source` so the
        // message stays meaningful regardless of which subsystem
        // registered the work. Falls back to "Loading" for unknown
        // sources.
        let elapsed = Instant::now().saturating_duration_since(ev.at);
        let kind = match ev.source {
            "drill-in" | "duplicate" => "Loading resource",
            "projection" => "Projecting",
            "compile" => "Compiling",
            "save" => "Saving",
            _ => "Loading",
        };
        let header = if elapsed < ELAPSED_AFTER {
            format!("{kind}…")
        } else if elapsed.as_secs_f32() < 10.0 {
            format!("{kind}… {:.1}s", elapsed.as_secs_f32())
        } else {
            format!("{kind}… {}s", elapsed.as_secs())
        };
        painter.text(
            egui::pos2(card_rect.min.x + 60.0, card_rect.center().y - 8.0),
            Align2::LEFT_CENTER,
            header,
            FontId::proportional(13.0),
            theme.tokens.text,
        );

        // Detail line — the bus event's message. Long qualified
        // names left-trimmed with an ellipsis so the leaf stays
        // visible.
        if !ev.message.is_empty() {
            let detail = if ev.message.len() > 40 {
                format!("…{}", &ev.message[ev.message.len() - 39..])
            } else {
                ev.message.clone()
            };
            painter.text(
                egui::pos2(card_rect.min.x + 60.0, card_rect.center().y + 10.0),
                Align2::LEFT_CENTER,
                detail,
                FontId::monospace(11.0),
                theme.tokens.text_subdued,
            );
        }

        // Cancel affordance — only when the originating task
        // registered a cancel flag via `StatusBus::begin_cancellable`.
        // Click flips the `AtomicBool` to `true`; the task's body
        // checks it at its cooperative checkpoints and short-circuits.
        if let Some(cancel) = ev.cancel.clone() {
            let btn_size = Vec2::new(20.0, 20.0);
            let btn_rect = Rect::from_min_size(
                egui::pos2(card_rect.max.x - btn_size.x - 8.0, card_rect.min.y + 8.0),
                btn_size,
            );
            // Use a fresh ui scoped to the button rect so we get an
            // egui-managed Response (hover, click, focus) rather than
            // hand-rolling hit-testing on the painter.
            let id = ui.id().with(("busy_cancel", ev.busy_id));
            let resp = ui.interact(btn_rect, id, egui::Sense::click());
            let bg = if resp.hovered() {
                theme.colors.surface1
            } else {
                theme.tokens.surface_raised
            };
            painter.rect_filled(btn_rect, 4.0, bg);
            painter.text(
                btn_rect.center(),
                Align2::CENTER_CENTER,
                "✕",
                FontId::proportional(13.0),
                theme.tokens.text_subdued,
            );
            if resp.clicked() {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    /// Inline indicator suitable for a tree row (no scrim, no card).
    /// Renders as `⌛ <label>` in a muted colour. No-op when not busy.
    pub fn inline(self, ui: &mut egui::Ui, bus: &StatusBus, theme: &Theme) {
        let Some(ev) = self.pick(bus) else { return };
        let label = if ev.message.is_empty() {
            "Loading…".to_string()
        } else {
            format!("⌛ {}", ev.message)
        };
        ui.colored_label(theme.colors.subtext0, label);
        ui.ctx().request_repaint();
    }

    /// Top-of-panel banner suitable for document-scope work. No-op
    /// when the scope is not busy. Reserved for Phase 3+ writers.
    pub fn banner(self, ui: &mut egui::Ui, bus: &StatusBus, theme: &Theme) {
        let Some(ev) = self.pick(bus) else { return };
        egui::Frame::new()
            .fill(theme.colors.surface0)
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("⌛").color(theme.colors.text));
                    let label = if ev.message.is_empty() {
                        "Loading…"
                    } else {
                        ev.message.as_str()
                    };
                    ui.colored_label(theme.colors.text, label);
                });
            });
        ui.ctx().request_repaint();
    }
}
