//! Modelica-specific theme helpers.

use bevy_egui::egui::Color32;
use lunco_theme::Theme;

/// Helper to get Modelica-specific tokens from the global theme.
pub trait ModelicaThemeExt {
    fn port_input(&self) -> Color32;
    fn port_output(&self) -> Color32;
    fn selection(&self) -> Color32;
    fn connection(&self) -> Color32;
}

impl ModelicaThemeExt for Theme {
    fn port_input(&self) -> Color32 {
        self.get_token("modelica", "port_input", self.colors.blue)
    }

    fn port_output(&self) -> Color32 {
        self.get_token("modelica", "port_output", self.colors.green)
    }

    fn selection(&self) -> Color32 {
        self.get_token("modelica", "selection", self.colors.mauve)
    }

    fn connection(&self) -> Color32 {
        self.get_token("modelica", "connection", self.colors.subtext0)
    }
}
