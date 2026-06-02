//! Modelica-diagram colour accessors.
//!
//! The canvas paint code runs partly outside the Bevy world (leaf
//! paint helpers called from inside egui callbacks), so it can't read
//! [`lunco_theme::Theme`] directly. The active theme is published once
//! per frame into egui's data cache by the render entry
//! ([`lunco_theme::store_active`]); the helpers here read it back via
//! [`lunco_theme::active`] and project it into diagram-semantic names.
//!
//! These are pure *views* over the single active theme — no separate
//! theme storage, no parallel palette. The only Modelica-specific
//! mapping (e.g. "port fill = `overlay1`") lives here, in one place.

use bevy_egui::egui;

/// Theme colours projected into Modelica-diagram semantics, derived
/// fresh from the active theme. Leaf paint helpers read these by name
/// (`port_fill`, `select_stroke`, …) without taking a `Res<Theme>`.
#[derive(Clone, Copy, Debug)]
pub struct CanvasThemeSnapshot {
    pub card_fill: egui::Color32,
    pub node_label: egui::Color32,
    pub type_label: egui::Color32,
    pub port_fill: egui::Color32,
    pub port_stroke: egui::Color32,
    pub select_stroke: egui::Color32,
    pub inactive_stroke: egui::Color32,
    pub icon_only_stroke: egui::Color32,
    /// When false (default), authored MSL icons render without a
    /// workbench-drawn hairline frame around them. The icon's own
    /// primitives are the bounds. Selection / icon-only / expandable
    /// rings still draw — they carry semantic info, not just bounds.
    pub show_authored_icon_border: bool,
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
            // the theme's dedicated `canvas_card` schematic token.
            card_fill: s.canvas_card,
            node_label: t.text,
            type_label: t.text_subdued,
            port_fill: c.overlay1,
            port_stroke: c.surface2,
            select_stroke: t.accent,
            inactive_stroke: c.overlay0,
            icon_only_stroke: t.warning,
            show_authored_icon_border: false,
        }
    }
}

/// Modelica-diagram colours derived from the theme published for this
/// frame (or `Theme::dark()` outside our panel — tests / demos).
pub(super) fn canvas_theme_from_ctx(ctx: &egui::Context) -> CanvasThemeSnapshot {
    CanvasThemeSnapshot::from_theme(&lunco_theme::active(ctx))
}

/// The active theme's MSL icon-remap palette, read from this frame's
/// published theme.
pub(super) fn modelica_icon_palette_from_ctx(
    ctx: &egui::Context,
) -> Option<lunco_theme::ModelicaIconPalette> {
    Some(lunco_theme::active(ctx).modelica_icons.clone())
}
