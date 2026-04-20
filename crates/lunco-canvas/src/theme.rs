//! Canvas-layer colour theme — per-frame overridable.
//!
//! `lunco-canvas` stays domain-agnostic (no `lunco-theme` dep), but
//! still needs its built-in layers (grid, selection halo, tool
//! preview, zoom-bar overlay) to render in colours that match the
//! embedding app's active palette. Rather than coupling the crate to
//! a specific theme source, we let the consumer *push* a
//! [`CanvasLayerTheme`] into the egui data cache before each render;
//! layers then pull it out on draw, falling back to sensible
//! dark-mode defaults when unset (unit tests, standalone demos).
//!
//! Usage from the consumer (once per frame, before `canvas.ui(...)`):
//!
//! ```ignore
//! lunco_canvas::theme::store(ui.ctx(), CanvasLayerTheme {
//!     grid: my_theme.grid_dot(),
//!     selection_outline: my_theme.accent(),
//!     ..Default::default()
//! });
//! ```

use bevy_egui::egui::{self, Color32};

/// Palette used by [`crate::GridLayer`], [`crate::SelectionLayer`],
/// [`crate::ToolPreviewLayer`], and [`crate::NavBarOverlay`].
///
/// Field values are direct `Color32`s so the canvas crate never needs
/// to know about the consumer's theme model.
#[derive(Clone, Copy, Debug)]
pub struct CanvasLayerTheme {
    /// Dotted world-grid colour. Typically a low-alpha overlay-tier
    /// shade so the grid reads at any zoom without competing with
    /// diagram content.
    pub grid: Color32,
    /// Selection halo stroke.
    pub selection_outline: Color32,
    /// Ghost-edge line colour during drag-to-connect.
    pub ghost_edge: Color32,
    /// Snap-target highlight ring during drag-to-connect.
    pub snap_target: Color32,
    /// Rubber-band marquee fill (semi-transparent).
    pub rubber_band_fill: Color32,
    /// Rubber-band marquee stroke.
    pub rubber_band_stroke: Color32,
    /// Overlay card fill (zoom bar, future HUDs).
    pub overlay_fill: Color32,
    /// Overlay card stroke.
    pub overlay_stroke: Color32,
    /// Overlay shadow colour (alpha baked in).
    pub overlay_shadow: Color32,
    /// Overlay primary text / label colour.
    pub overlay_text: Color32,
}

impl Default for CanvasLayerTheme {
    fn default() -> Self {
        Self {
            grid: Color32::from_rgba_premultiplied(60, 60, 72, 50),
            selection_outline: Color32::from_rgb(120, 170, 255),
            ghost_edge: Color32::from_rgb(140, 200, 255),
            snap_target: Color32::from_rgb(90, 220, 140),
            rubber_band_fill: Color32::from_rgba_premultiplied(120, 170, 255, 30),
            rubber_band_stroke: Color32::from_rgb(120, 170, 255),
            overlay_fill: Color32::from_rgb(34, 38, 48),
            overlay_stroke: Color32::from_rgb(60, 65, 78),
            overlay_shadow: Color32::from_rgba_premultiplied(0, 0, 0, 80),
            overlay_text: Color32::from_rgb(200, 210, 225),
        }
    }
}

fn theme_id() -> egui::Id {
    egui::Id::new("lunco_canvas.layer_theme")
}

/// Store the layer theme for this frame. Call once per frame from the
/// embedding app before `canvas.ui(...)`.
pub fn store(ctx: &egui::Context, theme: CanvasLayerTheme) {
    ctx.data_mut(|d| d.insert_temp(theme_id(), theme));
}

/// Read the active layer theme, or a dark-mode default if no consumer
/// has pushed one this frame.
pub fn current(ctx: &egui::Context) -> CanvasLayerTheme {
    ctx.data(|d| d.get_temp::<CanvasLayerTheme>(theme_id()))
        .unwrap_or_default()
}
