//! Input overlay panel rendering active keys and mouse actions in real-time.
//!
//! Visualizes simulator inputs for video generation or AI agent observation.
//! Persisted via `lunco-settings` under the `"input_overlay"` key.
//!
//! **Off by default, opted into per scenario.** This answers "which keys is the
//! demonstrator pressing", which is a question only a TEACHING context asks — a
//! tutorial beat, or a recorded shot where someone is actively flying. Everywhere
//! else it is chrome: it occupies the bottom of frame, it is baked into every
//! captured frame by the offline recorder, and on a shot where nobody is touching
//! the controls it shows an empty keyboard, which is worse than absent.
//!
//! Contrast with the vehicle HUD (`lunco-sandbox`'s rover HUD), which is NOT gated:
//! attitude, speed and telemetry describe the VEHICLE and are wanted whenever one is
//! on screen. This describes the OPERATOR, and is only wanted when the operator is
//! the subject.
//!
//! Turn it on with `ToggleInputOverlay { enabled: true }`.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use lunco_core::{Command, on_command, register_commands};

/// Persisted settings for the input overlay HUD.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub struct InputOverlaySettings {
    /// Whether the overlay is rendered.
    pub enabled: bool,
}

impl Default for InputOverlaySettings {
    fn default() -> Self {
        // OFF. A key-press readout is only meaningful while someone is demonstrating
        // controls; the rest of the time it is burnt-in chrome, and the offline
        // recorder captures the whole window, so every frame of every shot carries it.
        Self { enabled: false }
    }
}

// DELIBERATELY NOT a `SettingsSection` — this is not a user preference.
//
// Whether the key readout is on belongs to the SCENE: a tutorial teaching the
// controls wants it, a landing film does not. Persisting it makes that choice
// outlive the scene that made it, so a tutorial run turns it on and every later
// session — including an unattended recording — inherits it. That is a leak
// across scenes and across sessions, and it is invisible until it shows up baked
// into finished footage.
//
// So it resets to off on every launch and is opted into per scene, exactly like
// the camera paths and the HUD state a scenario drives. A setting persists when
// the USER owns the choice; this one the CONTENT owns.

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
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    if !settings.enabled {
        return;
    }
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let Ok(window) = windows.single() else { return };

    let panel_w = 340.0;
    let panel_h = 45.0;
    let x = (window.width() - panel_w) / 2.0;
    let y = window.height() - panel_h - 20.0;

    egui::Area::new(egui::Id::new("lunco_input_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(x, y))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(15, 23, 42, 220)) // Slate 900
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(99, 102, 241))) // Indigo 500
                .inner_margin(egui::Margin::symmetric(12, 8))
                .corner_radius(6.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Keyboard Key visualizer
                        let draw_key = |ui: &mut egui::Ui, text: &str, is_pressed: bool| {
                            let color = if is_pressed {
                                egui::Color32::from_rgb(129, 140, 248) // Active Indigo
                            } else {
                                egui::Color32::from_rgb(71, 85, 105) // Inactive Gray
                            };
                            ui.colored_label(color, egui::RichText::new(text).strong().size(13.0));
                        };

                        ui.label(egui::RichText::new("⌨").size(15.0).weak());
                        draw_key(ui, "W", keys.pressed(KeyCode::KeyW));
                        draw_key(ui, "A", keys.pressed(KeyCode::KeyA));
                        draw_key(ui, "S", keys.pressed(KeyCode::KeyS));
                        draw_key(ui, "D", keys.pressed(KeyCode::KeyD));
                        ui.separator();
                        draw_key(ui, "Space", keys.pressed(KeyCode::Space));
                        draw_key(ui, "Shift", keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight));
                        ui.separator();

                        // Mouse visualizer
                        let cursor_pos = window.cursor_position().unwrap_or(Vec2::ZERO);
                        let m_left = buttons.pressed(MouseButton::Left);
                        let m_right = buttons.pressed(MouseButton::Right);
                        let m_middle = buttons.pressed(MouseButton::Middle);
                        draw_key(ui, "🖱 L", m_left);
                        draw_key(ui, "M", m_middle);
                        draw_key(ui, "R", m_right);

                        ui.label(egui::RichText::new(format!(" [{:.0}, {:.0}]", cursor_pos.x, cursor_pos.y)).weak().size(10.0));
                    });
                });
        });
}

register_commands!(on_toggle_input_overlay);

/// Registers the input overlay resources, settings, commands, and systems.
pub fn build_input_overlay(app: &mut App) {
    // No `register_settings_section` — see the note on `InputOverlaySettings`. The
    // resource starts at its `Default` (off) on every launch, so nothing a previous
    // scene or session did can turn it on here.
    app.init_resource::<InputOverlaySettings>();
    app.add_systems(bevy_egui::EguiPrimaryContextPass, draw_input_overlay);
    
    register_all_commands(app);
}
