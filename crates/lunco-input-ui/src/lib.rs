//! Reusable input-state presentation.
//!
//! The overlay is an application UI adapter over the generic input settings.
//! It does not translate input or own vessel control; `lunco-controller` and
//! avatar consumers remain the specialized input producers.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use lunco_core::{Command, on_command, register_commands};
use lunco_input_core::key_label;
use lunco_settings::{AppSettingsExt, SettingsSection};
use lunco_workbench_widgets::{UiIcon, paint_icon};
use serde::{Deserialize, Serialize};

/// Persisted settings for the input overlay HUD.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub struct InputOverlaySettings {
    /// Whether the overlay is rendered.
    #[serde(default)]
    pub enabled: Option<bool>,
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
    let new = Some(trigger.event().enabled);
    if settings.enabled != new {
        settings.enabled = new;
        info!("[input-overlay] set enabled to {}", new.unwrap_or(false));
    }
}

/// Draw the input overlay in the foreground of the primary egui context.
pub fn draw_input_overlay(
    mut egui_ctx: EguiContexts,
    settings: Res<InputOverlaySettings>,
    bindings: Res<lunco_input_core::ActiveInputBindings>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    pointer: lunco_interaction_core::PrimaryMousePointer,
    theme: Option<Res<lunco_theme::Theme>>,
    authority: Option<Res<lunco_core::markers::FlightAuthority>>,
) {
    let authority = authority.map(|a| *a).unwrap_or_default();
    if !settings.enabled.unwrap_or(false) {
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

    lunco_theme::HudTier::General
        .area(ctx, egui::Id::new("lunco_input_overlay"))
        .fixed_pos(egui::pos2(x, y))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme.tokens.overlay_backdrop)
                .stroke(egui::Stroke::new(1.0, theme.tokens.overlay_border))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .corner_radius(6.0)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
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
                                        egui::RichText::new(text)
                                            .strong()
                                            .text_style(
                                                lunco_theme::TypographyRole::Label.text_style(),
                                            )
                                            .color(glyph),
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
                            let pressed = binding.iter().any(|key| keys.pressed(*key));
                            draw_key(ui, &label, pressed);
                        }
                        ui.separator();

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
                                        .text_style(lunco_theme::TypographyRole::Label.text_style())
                                        .color(theme.tokens.overlay_backdrop),
                                );
                            });
                        ui.separator();

                        draw_key(ui, "L", buttons.pressed(MouseButton::Left));
                        draw_key(ui, "M", buttons.pressed(MouseButton::Middle));
                        draw_key(ui, "R", buttons.pressed(MouseButton::Right));
                        if let Some(cursor_pos) = pointer.position() {
                            ui.label(
                                egui::RichText::new(format!(
                                    " [{:.0}, {:.0}]",
                                    cursor_pos.x, cursor_pos.y
                                ))
                                .weak()
                                .text_style(lunco_theme::TypographyRole::DenseData.text_style()),
                            );
                        }
                    });
                });
        });
}

register_commands!(on_toggle_input_overlay,);

fn register_hud_menu(world: &mut World) {
    let Some(mut menus) = world.get_resource_mut::<lunco_workbench_core::WorkbenchMenuRegistry>()
    else {
        return;
    };
    menus.register_hud_menu(lunco_workbench_core::HudMenuGroup::BuiltIn, |ui, ctx| {
        let Some(mut settings) = ctx.resource::<InputOverlaySettings>().copied() else {
            return;
        };
        if let Some(choice) = (lunco_workbench_core::HudVisibility {
            enabled: settings.enabled.unwrap_or(false),
            available: true,
        })
        .menu_row(ui, "Input HUD", settings.enabled)
        .choice
        {
            settings.enabled = choice;
            ctx.set_resource(settings);
        }
    });
}

fn set_all_visibility(
    event: On<lunco_workbench_core::SetAllHudVisibility>,
    mut settings: ResMut<InputOverlaySettings>,
) {
    if settings.enabled != Some(event.0) {
        settings.enabled = Some(event.0);
    }
}

/// Register the typed presentation commands without installing the egui panel.
pub fn register_input_overlay_commands(app: &mut App) {
    app.register_settings_section::<InputOverlaySettings>();
    app.init_resource::<InputOverlaySettings>();
    app.init_resource::<lunco_core::markers::FlightAuthority>();
    app.add_systems(Startup, register_hud_menu)
        .add_observer(set_all_visibility);
    register_all_commands(app);
}

/// Register the input overlay resources, commands, and render system.
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
        assert!(!InputOverlaySettings::default().enabled.unwrap_or(false));
    }
}
