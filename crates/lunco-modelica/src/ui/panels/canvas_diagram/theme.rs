//! Canvas theme snapshot + helpers.
//!
//! The canvas paint code runs partly outside the Bevy world (leaf
//! paint helpers called from inside egui callbacks), so it can't read
//! [`lunco_theme::Theme`] directly. The render entry stashes a snapshot
//! into egui's data cache once per frame; leaf helpers fetch it from
//! the [`egui::Context`] via [`canvas_theme_from_ctx`].

use bevy_egui::egui;

/// Theme colors snapshotted once per frame so leaf paint helpers can
/// stay theme-aware without taking a `Res<Theme>` parameter.
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

/// Fetch the theme snapshot stored for this frame by the canvas
/// render entry. `None` when the canvas is rendered outside our
/// panel (tests / demos); caller falls back to a default snapshot
/// derived from `Theme::dark()`.
pub(super) fn canvas_theme_from_ctx(ctx: &egui::Context) -> CanvasThemeSnapshot {
    let id = egui::Id::new("lunco.modelica.canvas_theme_snapshot");
    ctx.data(|d| d.get_temp::<CanvasThemeSnapshot>(id))
        .unwrap_or_else(|| CanvasThemeSnapshot::from_theme(&lunco_theme::Theme::dark()))
}

/// Build the generic `lunco_canvas` layer theme (grid, selection halo,
/// tool preview, zoom-bar overlay) from the active LunCoSim theme.
/// Pushed to the canvas each frame so its built-in layers render in
/// palette-matched colours instead of their hardcoded dark defaults.
pub(super) fn layer_theme_from(theme: &lunco_theme::Theme) -> lunco_canvas::CanvasLayerTheme {
    let c = &theme.colors;
    let t = &theme.tokens;
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
        snap_guide: {
            let w = t.warning;
            egui::Color32::from_rgba_unmultiplied(w.r(), w.g(), w.b(), 180)
        },
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
pub(super) fn store_canvas_theme(ctx: &egui::Context, snap: CanvasThemeSnapshot) {
    let id = egui::Id::new("lunco.modelica.canvas_theme_snapshot");
    ctx.data_mut(|d| d.insert_temp(id, snap));
}

/// Stash the active theme's Modelica icon palette in the egui data
/// cache so leaf paint helpers (running outside the Bevy world) can
/// remap authored MSL colors to fit the active theme.
pub(super) fn store_modelica_icon_palette(
    ctx: &egui::Context,
    palette: lunco_theme::ModelicaIconPalette,
) {
    let id = egui::Id::new("lunco.modelica.icon_palette");
    ctx.data_mut(|d| d.insert_temp(id, palette));
}

pub(super) fn modelica_icon_palette_from_ctx(
    ctx: &egui::Context,
) -> Option<lunco_theme::ModelicaIconPalette> {
    let id = egui::Id::new("lunco.modelica.icon_palette");
    ctx.data(|d| d.get_temp::<lunco_theme::ModelicaIconPalette>(id))
}
