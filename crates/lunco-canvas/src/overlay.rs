//! Floating screen-space UI rendered on top of the scene.
//!
//! An [`Overlay`] is a post-pass that gets the canvas's widget rect
//! and the (non-mutable) canvas state. Unlike a [`crate::layer::Layer`]
//! it draws in **screen coordinates** (pinned to the widget corners,
//! not to the viewport) and it's free to consume pointer events over
//! its own rect — the canvas's input router asks overlays first.
//!
//! B1 ships **zero** concrete overlays — only the trait. The slot
//! exists so the following features land as single-file additions:
//!
//! - `MinimapOverlay` — miniature of the scene, draggable to pan.
//! - `NavBarOverlay` — zoom slider, fit-all button, breadcrumb.
//! - `StatsOverlay` — node/edge count, active tool, debug FPS.
//! - `SearchOverlay` — Ctrl+P to jump to a node by name.
//! - `PropertyPopoverOverlay` — hover-to-preview values, Figma-style.
//!
//! Each of these is roughly one file, one impl of this trait,
//! ~100-200 LOC. Nothing in the canvas core changes to add any of
//! them.

use bevy_egui::egui;

use crate::scene::{Rect, Scene};
use crate::selection::Selection;
use crate::viewport::Viewport;

/// Canvas state visible to overlays — read-only on scene/selection
/// so overlays can't accidentally mutate authored state; mutable
/// on viewport so a minimap can pan by dragging.
pub struct OverlayCtx<'a> {
    pub scene: &'a Scene,
    pub selection: &'a Selection,
    pub viewport: &'a mut Viewport,
    /// The widget rect the canvas is painting into — overlays anchor
    /// themselves to corners of this, not the window.
    pub canvas_screen_rect: Rect,
}

/// Floating UI layer. See module docs.
pub trait Overlay: Send + Sync {
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut OverlayCtx);
    fn name(&self) -> &'static str;
}

// ─── NavBarOverlay ─────────────────────────────────────────────────

/// Miro-style floating navigation controls — zoom in/out, zoom
/// percentage, fit-all. Anchored to the bottom-right of the canvas
/// widget by default; [`NavBarOverlay::anchor`] picks a different
/// corner.
///
/// Clicking +/- applies a fixed zoom step around the screen centre
/// (not the cursor), matching Miro/Figma nav-bar behaviour — the
/// cursor-pivot heuristic is for scroll, not button clicks.
///
/// "Fit" calls `viewport.set_target(fit_values(...))` and lets the
/// smooth-tick take over, so the move animates.
pub struct NavBarOverlay {
    pub anchor: Anchor,
    pub zoom_step: f32,
    pub fit_padding: f32,
    /// When true, the `%` readout and the `1:1` button are
    /// referenced to [`crate::Viewport::physical_mm_zoom`] — "100 %"
    /// means 1 world-mm = 1 screen-mm. When false, they're
    /// referenced to raw `zoom = 1.0`. Physical reference is the
    /// right default for Modelica (world units are mm); raw is
    /// right for pure node-graph editors where world units are
    /// just "logical" and have no physical meaning.
    pub use_physical_reference: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
}

impl Default for NavBarOverlay {
    fn default() -> Self {
        Self {
            anchor: Anchor::BottomRight,
            // 1.25× per click — feels right for +/- buttons. Scroll
            // wheel is much finer (see ViewportConfig::scroll_zoom_gain).
            zoom_step: 1.25,
            fit_padding: 40.0,
            use_physical_reference: true,
        }
    }
}

impl Overlay for NavBarOverlay {
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut OverlayCtx) {
        let canvas_rect = egui::Rect::from_min_max(
            egui::pos2(ctx.canvas_screen_rect.min.x, ctx.canvas_screen_rect.min.y),
            egui::pos2(ctx.canvas_screen_rect.max.x, ctx.canvas_screen_rect.max.y),
        );
        // Pick a corner. 12 px inset so the bar isn't flush with the
        // canvas edge — easier to grab the corner of a panel.
        let pad = 12.0;
        let bar_w = 240.0;
        let bar_h = 36.0;
        let (min, max) = match self.anchor {
            Anchor::BottomRight => {
                let max = canvas_rect.max - egui::vec2(pad, pad);
                (max - egui::vec2(bar_w, bar_h), max)
            }
            Anchor::BottomLeft => {
                let min = canvas_rect.min + egui::vec2(pad, canvas_rect.height() - bar_h - pad);
                (min, min + egui::vec2(bar_w, bar_h))
            }
            Anchor::TopRight => {
                let min = egui::pos2(canvas_rect.max.x - bar_w - pad, canvas_rect.min.y + pad);
                (min, min + egui::vec2(bar_w, bar_h))
            }
            Anchor::TopLeft => {
                let min = canvas_rect.min + egui::vec2(pad, pad);
                (min, min + egui::vec2(bar_w, bar_h))
            }
        };
        let bar_rect = egui::Rect::from_min_max(min, max);

        // Painter background: rounded rect with subtle shadow. No
        // real drop-shadow API in egui so we fake with a darker
        // rect offset by 2 px. Good enough for the first pass.
        let painter = ui.painter();
        painter.rect_filled(
            bar_rect.translate(egui::vec2(0.0, 2.0)),
            8.0,
            egui::Color32::from_rgba_premultiplied(0, 0, 0, 80),
        );
        painter.rect_filled(
            bar_rect,
            8.0,
            egui::Color32::from_rgb(34, 38, 48),
        );
        painter.rect_stroke(
            bar_rect,
            8.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 65, 78)),
            egui::StrokeKind::Outside,
        );

        // Put a child UI inside the bar for button widgets.
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(bar_rect.shrink(6.0))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);

        let zoom = ctx.viewport.zoom;

        // Zoom out
        if child
            .button(egui::RichText::new("−").size(16.0).monospace())
            .on_hover_text("Zoom out")
            .clicked()
        {
            let new_zoom = (zoom / self.zoom_step).max(ctx.viewport.config.zoom_min);
            ctx.viewport.set_target(ctx.viewport.center, new_zoom);
        }

        // Zoom % label referenced to physical mm (or raw, if the
        // app has opted out). Dymola / Figma / Illustrator all
        // display zoom as a ratio to a "natural" scale rather than
        // raw world→point, which is what the eye wants to see.
        let reference = if self.use_physical_reference {
            crate::Viewport::physical_mm_zoom(ui.ctx())
        } else {
            1.0
        };
        let zoom_pct = (zoom / reference.max(f32::EPSILON) * 100.0).round() as i32;
        child.add_sized(
            egui::vec2(56.0, 22.0),
            egui::Label::new(
                egui::RichText::new(format!("{}%", zoom_pct))
                    .monospace()
                    .color(egui::Color32::from_rgb(200, 210, 225)),
            ),
        );

        // Zoom in
        if child
            .button(egui::RichText::new("+").size(16.0).monospace())
            .on_hover_text("Zoom in")
            .clicked()
        {
            let new_zoom = (zoom * self.zoom_step).min(ctx.viewport.config.zoom_max);
            ctx.viewport.set_target(ctx.viewport.center, new_zoom);
        }

        child.separator();

        // Reset to 100 %. With `use_physical_reference = true`,
        // that lands at physical scale (1 world-mm = 1 screen-mm).
        if child
            .button(egui::RichText::new("1:1").size(12.0))
            .on_hover_text("Reset zoom to 100 %")
            .clicked()
        {
            let target = if self.use_physical_reference {
                crate::Viewport::physical_mm_zoom(ui.ctx())
            } else {
                1.0
            };
            ctx.viewport.set_target(ctx.viewport.center, target);
        }

        // Fit all
        if child
            .button(egui::RichText::new("Fit").size(12.0))
            .on_hover_text("Fit the whole scene (F)")
            .clicked()
        {
            if let Some(bounds) = ctx.scene.bounds() {
                let (c, z) = ctx
                    .viewport
                    .fit_values(bounds, ctx.canvas_screen_rect, self.fit_padding);
                ctx.viewport.set_target(c, z);
            }
        }
    }

    fn name(&self) -> &'static str {
        "navbar"
    }
}
