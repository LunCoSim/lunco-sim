//! Input overlay panel rendering active keys and mouse actions in real-time.
//!
//! Visualizes simulator inputs for video generation or AI agent observation.
//! Persisted via `lunco-settings` under the `"input_overlay"` key.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use lunco_controller::{key_label, InputBindingsSettings};
use lunco_core::{on_command, register_commands, Command};
use lunco_settings::{AppSettingsExt, SettingsSection};
use lunco_workbench_widgets::{paint_icon, UiIcon};
use serde::{Deserialize, Serialize};

/// Persisted settings for the input overlay HUD.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub struct InputOverlaySettings {
    /// Whether the overlay is rendered.
    pub enabled: bool,
}

impl SettingsSection for InputOverlaySettings {
    const KEY: &'static str = "input_overlay";
}

/// Command to toggle the input overlay visibility.
#[Command(default)]
pub struct ToggleInputOverlay {
    /// `true` to show the overlay, `false` to hide it.
    pub enabled: bool,
}

#[on_command(ToggleInputOverlay)]
fn on_toggle_input_overlay(
    trigger: On<ToggleInputOverlay>,
    mut settings: ResMut<InputOverlaySettings>,
) {
    let new = trigger.event().enabled;
    if settings.enabled != new {
        settings.enabled = new;
        info!("[input-overlay] set enabled to {new}");
    }
}

/// System to draw the input overlay HUD in the foreground of the primary egui context.
pub fn draw_input_overlay(
    mut egui_ctx: EguiContexts,
    settings: Res<InputOverlaySettings>,
    bindings: Res<InputBindingsSettings>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    theme: Option<Res<lunco_theme::Theme>>,
    authority: Option<Res<lunco_core::markers::FlightAuthority>>,
) {
    let authority = authority.map(|a| *a).unwrap_or_default();
    if !settings.enabled {
        return;
    }
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let Ok(window) = windows.single() else { return };
    let theme = theme
        .map(|t| t.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);

    let panel_w = 700.0;
    let panel_h = 86.0;
    let x = (window.width() - panel_w) / 2.0;
    let y = window.height() - panel_h - 20.0;

    egui::Area::new(egui::Id::new("lunco_input_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(x, y))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme.tokens.overlay_backdrop)
                .stroke(egui::Stroke::new(1.0, theme.tokens.overlay_border))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .corner_radius(6.0)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        // Keyboard key visualizer, drawn as KEYCAP CHIPS. A pressed
                        // key fills its chip with the warning (amber) token and
                        // flips the glyph dark — a colored-text-only pressed state
                        // (the previous styling) was invisible at video scale over
                        // dark footage, which defeats the overlay's whole purpose
                        // (it exists FOR the recordings).
                        let draw_key = |ui: &mut egui::Ui, text: &str, is_pressed: bool| {
                            let (fill, glyph, border) = if is_pressed {
                                (
                                    theme.tokens.warning,
                                    theme.tokens.overlay_backdrop,
                                    theme.tokens.warning,
                                )
                            } else {
                                (
                                    egui::Color32::TRANSPARENT,
                                    theme.tokens.inactive,
                                    theme.tokens.overlay_border,
                                )
                            };
                            egui::Frame::new()
                                .fill(fill)
                                .stroke(egui::Stroke::new(1.0, border))
                                .corner_radius(4.0)
                                .inner_margin(egui::Margin::symmetric(7, 3))
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new(text).strong().size(15.0).color(glyph),
                                    );
                                });
                        };

                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                        paint_icon(
                            ui.painter(),
                            UiIcon::Keyboard,
                            rect,
                            ui.visuals().weak_text_color(),
                        );
                        let Ok(bindings) = bindings.key_bindings() else {
                            error!("[input-overlay] active keymap is invalid");
                            return;
                        };
                        for (_, binding) in bindings {
                            let label = key_label(&binding);
                            let pressed = binding
                                .iter()
                                .any(|key| keys.pressed(*key));
                            draw_key(ui, &label, pressed);
                        }
                        ui.separator();

                        // WHO IS FLYING. A key row shows inputs arriving; it
                        // cannot show whether they are being obeyed. `piloted`
                        // is the vessel's own authority gate (1 = a session has
                        // the stick, 0 = the guidance law flies), so this badge
                        // is the state itself rather than a caption about it —
                        // and it is what makes a handback legible: the keys go
                        // dark, and MANUAL flips to AUTO in the same frame.
                        let (mode, mode_color) = if authority.piloted {
                            ("MANUAL", theme.tokens.warning)
                        } else {
                            ("AUTO", theme.tokens.success)
                        };
                        egui::Frame::new()
                            .fill(mode_color)
                            .corner_radius(4.0)
                            .inner_margin(egui::Margin::symmetric(8, 3))
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(mode)
                                        .strong()
                                        .size(14.0)
                                        .color(theme.tokens.overlay_backdrop),
                                );
                            });
                        ui.separator();

                        // Mouse visualizer
                        let cursor_pos = window.cursor_position().unwrap_or(Vec2::ZERO);
                        let m_left = buttons.pressed(MouseButton::Left);
                        let m_right = buttons.pressed(MouseButton::Right);
                        let m_middle = buttons.pressed(MouseButton::Middle);
                        draw_key(ui, "L", m_left);
                        draw_key(ui, "M", m_middle);
                        draw_key(ui, "R", m_right);

                        ui.label(
                            egui::RichText::new(format!(
                                " [{:.0}, {:.0}]",
                                cursor_pos.x, cursor_pos.y
                            ))
                            .weak()
                            .size(10.0),
                        );
                    });
                });
        });
}

register_commands!(
    on_toggle_input_overlay,
);

/// Register the typed presentation commands without installing an egui panel.
/// Offscreen/headless scenario runners still receive the shared command surface.
pub fn register_input_overlay_commands(app: &mut App) {
    app.register_settings_section::<InputOverlaySettings>();
    app.init_resource::<InputOverlaySettings>();
    app.init_resource::<InputBindingsSettings>();
    app.init_resource::<lunco_core::markers::FlightAuthority>();
    register_all_commands(app);
}

/// Registers the input overlay resources, settings, commands, and systems.
pub fn build_input_overlay(app: &mut App) {
    register_input_overlay_commands(app);
    app.add_systems(
        bevy_egui::EguiPrimaryContextPass,
        draw_input_overlay.in_set(lunco_workbench_core::ApplicationOverlayRenderSet),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_overlay_settings_use_the_shared_persisted_section() {
        assert_eq!(InputOverlaySettings::KEY, "input_overlay");
        assert_eq!(
            serde_json::to_value(InputOverlaySettings::default()).unwrap(),
            serde_json::json!({"enabled": false})
        );
    }
}
