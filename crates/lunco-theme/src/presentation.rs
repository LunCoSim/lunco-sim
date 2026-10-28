//! Shared presentation preferences for engineering diagram hosts and widgets.
use bevy::prelude::*;
use bevy_egui::egui;

/// Hiding controls leaves authored topology, diagrams and live values intact.
/// The central settings owner persists this preference, never model source.
#[derive(Resource, Reflect, serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
#[reflect(Resource)]
#[serde(default)]
pub struct DiagramPresentationSettings {
    pub show_controls: bool,
    /// Reveal nested Modelica diagrams as components grow on screen.
    pub semantic_zoom: bool,
    /// Preserve deliberate authored backgrounds/text instead of the source
    /// library's automatic dark-theme color inversion.
    pub use_authored_colors: bool,
    /// Detailed pointer-hover cards can obscure a diagram during playback.
    pub show_hover_values: bool,
}

impl Default for DiagramPresentationSettings {
    fn default() -> Self {
        Self {
            show_controls: true,
            semantic_zoom: false,
            use_authored_colors: false,
            show_hover_values: true,
        }
    }
}

pub fn diagram_hover_values(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(egui::Id::new("lunco_theme.diagram_hover_values")))
        .unwrap_or(true)
}

impl lunco_settings::SettingsSection for DiagramPresentationSettings {
    const KEY: &'static str = "diagram_presentation";
}

/// Preserve diagram geometry at deep zoom without allocating enormous glyphs.
/// Raster size is bounded in physical pixels; epaint's supported mesh transform
/// scales the galley afterward. This is a rendering boundary, not a font-size
/// clamp on authored Modelica annotations.
pub fn diagram_text_galley(
    painter: &egui::Painter,
    text: String,
    mut font: egui::FontId,
    color: egui::Color32,
) -> std::sync::Arc<egui::Galley> {
    let raster_limit = 128.0 / painter.ctx().pixels_per_point().max(1.0);
    let scale = (font.size / raster_limit).max(1.0);
    font.size /= scale;
    let galley = painter.layout_no_wrap(text, font, color);
    if scale == 1.0 {
        return galley;
    }
    let mut shape = egui::epaint::TextShape::new(egui::Pos2::ZERO, galley, color);
    shape.transform(egui::emath::TSTransform::from_scaling(scale));
    shape.galley
}
