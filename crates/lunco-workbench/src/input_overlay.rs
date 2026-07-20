//! Input overlay panel rendering active keys and mouse actions in real-time.
//!
//! Visualizes simulator inputs for video generation or AI agent observation.
//! Persisted via `lunco-settings` under the `"input_overlay"` key.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use lunco_core::{Command, on_command, register_commands};
use std::collections::HashSet;

/// Persisted settings for the input overlay HUD.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
#[derive(Default)]
pub struct InputOverlaySettings {
    /// Whether the overlay is rendered.
    pub enabled: bool,
}

/// Simulated inputs from scripts or playback.
#[derive(Resource, Default, Clone, Debug)]
pub struct SimulatedInputs {
    pub keys: HashSet<KeyCode>,
}

/// Command to toggle the input overlay visibility.
#[Command(default)]
pub struct ToggleInputOverlay {
    /// `true` to show the overlay, `false` to hide it.
    pub enabled: bool,
}

/// Command to simulate a keyboard input for the overlay.
#[Command(default)]
pub struct SimulateInput {
    pub key: String,
    pub pressed: bool,
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

#[on_command(SimulateInput)]
fn on_simulate_input(
    trigger: On<SimulateInput>,
    mut simulated: ResMut<SimulatedInputs>,
) {
    let cmd = trigger.event();
    // Every key the vessel control profile actually binds. The old list stopped
    // at W/A/S/D/Space/Shift, so `SimulateInput` for anything else was accepted
    // and silently dropped — most visibly `G`, the release-to-autopilot key: a
    // scripted handback fired the command, changed the flight authority, and
    // showed the viewer nothing at all.
    let code = match cmd.key.as_str() {
        "W" | "w" => Some(KeyCode::KeyW),
        "A" | "a" => Some(KeyCode::KeyA),
        "S" | "s" => Some(KeyCode::KeyS),
        "D" | "d" => Some(KeyCode::KeyD),
        "Q" | "q" => Some(KeyCode::KeyQ),
        "E" | "e" => Some(KeyCode::KeyE),
        "G" | "g" => Some(KeyCode::KeyG),
        "R" | "r" => Some(KeyCode::KeyR),
        "L" | "l" => Some(KeyCode::KeyL),
        "M" | "m" => Some(KeyCode::KeyM),
        "Space" | "space" => Some(KeyCode::Space),
        "Shift" | "shift" => Some(KeyCode::ShiftLeft),
        _ => {
            warn!("[input-overlay] SimulateInput: unmapped key {:?} — ignored", cmd.key);
            None
        }
    };
    if let Some(c) = code {
        if cmd.pressed {
            simulated.keys.insert(c);
        } else {
            simulated.keys.remove(&c);
        }
    }
}

/// System to draw the input overlay HUD in the foreground of the primary egui context.
pub fn draw_input_overlay(
    mut egui_ctx: EguiContexts,
    settings: Res<InputOverlaySettings>,
    simulated: Res<SimulatedInputs>,
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

    let panel_w = 430.0;
    let panel_h = 52.0;
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
                    ui.horizontal(|ui| {
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
                                        egui::RichText::new(text)
                                            .strong()
                                            .size(15.0)
                                            .color(glyph),
                                    );
                                });
                        };

                        ui.label(egui::RichText::new("⌨").size(15.0).weak());
                        draw_key(ui, "W", keys.pressed(KeyCode::KeyW) || simulated.keys.contains(&KeyCode::KeyW));
                        draw_key(ui, "A", keys.pressed(KeyCode::KeyA) || simulated.keys.contains(&KeyCode::KeyA));
                        draw_key(ui, "S", keys.pressed(KeyCode::KeyS) || simulated.keys.contains(&KeyCode::KeyS));
                        draw_key(ui, "D", keys.pressed(KeyCode::KeyD) || simulated.keys.contains(&KeyCode::KeyD));
                        ui.separator();
                        draw_key(ui, "Space", keys.pressed(KeyCode::Space) || simulated.keys.contains(&KeyCode::Space));
                        draw_key(ui, "Shift", keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) || simulated.keys.contains(&KeyCode::ShiftLeft));
                        // Q/E yaw and G (release to autopilot) are bound by the
                        // vessel control profile and were missing from the row —
                        // so the single most important keystroke in a piloted
                        // landing, the handback, was invisible.
                        draw_key(ui, "Q", keys.pressed(KeyCode::KeyQ) || simulated.keys.contains(&KeyCode::KeyQ));
                        draw_key(ui, "E", keys.pressed(KeyCode::KeyE) || simulated.keys.contains(&KeyCode::KeyE));
                        draw_key(ui, "G", keys.pressed(KeyCode::KeyG) || simulated.keys.contains(&KeyCode::KeyG));
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
                        draw_key(ui, "🖱 L", m_left);
                        draw_key(ui, "M", m_middle);
                        draw_key(ui, "R", m_right);

                        ui.label(egui::RichText::new(format!(" [{:.0}, {:.0}]", cursor_pos.x, cursor_pos.y)).weak().size(10.0));
                    });
                });
        });
}

register_commands!(on_toggle_input_overlay, on_simulate_input);

/// Registers the input overlay resources, settings, commands, and systems.
pub fn build_input_overlay(app: &mut App) {
    app.init_resource::<InputOverlaySettings>();
    app.init_resource::<SimulatedInputs>();
    // Who is flying — written by possess/release, read by the AUTO/MANUAL badge.
    app.init_resource::<lunco_core::markers::FlightAuthority>();
    app.add_systems(bevy_egui::EguiPrimaryContextPass, draw_input_overlay);
    
    register_all_commands(app);
}
