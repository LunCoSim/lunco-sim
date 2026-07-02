//! Script-driven tutorial overlay: a persistent objectives/hint HUD plus a
//! widget spotlight. Both are the *display surface* the tutorial system was
//! missing — [`ShowNotification`](lunco_avatar) toasts fade, but a tutorial
//! needs sticky instructions and a way to point at a widget.
//!
//! Everything here is driven by **commands** (API- and rhai-callable), so a
//! `.rhai` tutorial script puts instructions on screen with no Rust:
//!
//! ```rhai
//! hint("Press F to take control of the rover");
//! objectives_hud([ #{ text: "Reach the flag", state: "active" } ]);
//! spotlight("twin_browser", "Your models live here");
//! clear_spotlight();
//! ```
//!
//! Lives in `lunco-workbench` (not a tutorial-only crate) because both the
//! sandbox and the lunica Modelica workbench load `WorkbenchPlugin`, so the
//! same HUD is available to every app. The [`HelpAnchors`](crate::HelpAnchors)
//! rect registry it spotlights against already lives here too.
//!
//! Command payloads are single strings (objectives arrive pre-formatted as a
//! checklist block from the rhai prelude) — the same trivially-marshalled shape
//! as `ShowNotification.text`, avoiding any nested-collection reflection.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use lunco_core::{on_command, register_commands, Command};

/// Persistent tutorial HUD + spotlight state. Always present (headless too) so
/// the commands never panic on a missing resource; only the draw is ui-gated.
#[derive(Resource, Default, Clone, Debug)]
pub struct TutorialHud {
    /// One-line instruction shown at the top of the HUD card. Empty = hidden.
    pub hint: String,
    /// Pre-formatted objectives checklist block (one objective per line, with a
    /// leading glyph). Empty = the objectives card is hidden.
    pub objectives: String,
    /// Active spotlight: `(anchor_key, caption)`. `anchor_key` resolves against
    /// [`HelpAnchors`](crate::HelpAnchors); an unknown/absent key still dims the
    /// screen and shows a centred caption. `None` = no spotlight.
    pub spotlight: Option<(String, String)>,
}

// ── Commands ────────────────────────────────────────────────────────────────

/// Set the persistent one-line hint. Empty `text` clears it. Rhai: `hint(msg)`
/// / `clear_hint()`.
#[Command(default)]
pub struct SetHint {
    /// Instruction text; empty hides the hint line.
    pub text: String,
}

/// Set the persistent objectives checklist. `text` is a pre-formatted block
/// (one objective per line). Empty clears it. Rhai: `objectives_hud(list)` —
/// the prelude formats the list into this block and also auto-publishes it from
/// declarative `mission(me)` state.
#[Command(default)]
pub struct SetObjectives {
    /// Pre-formatted checklist block; empty hides the objectives card.
    pub text: String,
}

/// Spotlight a workbench widget by its [`HelpAnchors`](crate::HelpAnchors) key,
/// dimming everything else. Rhai: `spotlight(anchor, caption)`.
#[Command(default)]
pub struct Spotlight {
    /// The `HelpAnchors` key of the widget to highlight (e.g. `"twin_browser"`).
    pub anchor: String,
    /// Optional caption shown in the callout. Empty = no caption text.
    #[serde(default)]
    #[reflect(default)]
    pub text: String,
}

/// Clear any active spotlight. Rhai: `clear_spotlight()`.
#[Command(default)]
pub struct ClearSpotlight {}

#[on_command(SetHint)]
fn on_set_hint(cmd: SetHint, mut hud: ResMut<TutorialHud>) {
    hud.hint = cmd.text.clone();
}

#[on_command(SetObjectives)]
fn on_set_objectives(cmd: SetObjectives, mut hud: ResMut<TutorialHud>) {
    hud.objectives = cmd.text.clone();
}

#[on_command(Spotlight)]
fn on_spotlight(cmd: Spotlight, mut hud: ResMut<TutorialHud>) {
    hud.spotlight = Some((cmd.anchor.clone(), cmd.text.clone()));
}

#[on_command(ClearSpotlight)]
fn on_clear_spotlight(_cmd: ClearSpotlight, mut hud: ResMut<TutorialHud>) {
    hud.spotlight = None;
}

register_commands!(on_set_hint, on_set_objectives, on_spotlight, on_clear_spotlight,);

// ── Rendering ─────────────────────────────────────────────────────────────

const ACCENT: egui::Color32 = egui::Color32::from_rgb(90, 170, 255);

/// Draw the persistent objectives/hint card, top-left, below the menu bar.
/// Non-interactive (foreground layer) so it never eats clicks.
fn draw_tutorial_hud(mut egui_ctx: EguiContexts, hud: Res<TutorialHud>) {
    if hud.hint.is_empty() && hud.objectives.is_empty() {
        return;
    }
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let screen = ctx.content_rect();

    egui::Area::new(egui::Id::new("lunco_tutorial_hud"))
        .order(egui::Order::Foreground)
        .interactable(false)
        .fixed_pos(egui::pos2(screen.left() + 16.0, screen.top() + 44.0))
        .show(ctx, |ui| {
            ui.set_max_width(320.0);
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(18, 24, 38, 235))
                .corner_radius(10.0)
                .stroke(egui::Stroke::new(1.0, ACCENT.linear_multiply(0.6)))
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    if !hud.objectives.is_empty() {
                        ui.label(
                            egui::RichText::new("OBJECTIVES")
                                .color(ACCENT)
                                .small()
                                .strong(),
                        );
                        ui.add_space(2.0);
                        for line in hud.objectives.lines() {
                            // Colour done/failed lines by their leading glyph.
                            let color = match line.chars().next() {
                                Some('✓') => egui::Color32::from_rgb(140, 230, 160),
                                Some('✗') => egui::Color32::from_rgb(240, 150, 150),
                                Some('▸') => egui::Color32::from_rgb(230, 235, 245),
                                _ => egui::Color32::from_rgb(160, 172, 190),
                            };
                            ui.label(egui::RichText::new(line).color(color).size(14.0));
                        }
                    }
                    if !hud.hint.is_empty() {
                        if !hud.objectives.is_empty() {
                            ui.add_space(6.0);
                            ui.separator();
                            ui.add_space(4.0);
                        }
                        ui.label(
                            egui::RichText::new(&hud.hint)
                                .color(egui::Color32::from_rgb(210, 224, 245))
                                .size(15.0),
                        );
                    }
                });
        });
}

/// Draw the spotlight: dim the screen except the anchored widget's rect, ring
/// it with a pulsing accent, and show a caption callout. Falls back to a full
/// dim + centred caption when the anchor isn't currently painted.
fn draw_spotlight(mut egui_ctx: EguiContexts, hud: Res<TutorialHud>, anchors: Res<crate::HelpAnchors>) {
    let Some((key, caption)) = hud.spotlight.clone() else { return };
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let screen = ctx.content_rect();
    let target = anchors.get(&key);

    egui::Area::new(egui::Id::new("lunco_spotlight_scrim"))
        .order(egui::Order::Foreground)
        .interactable(false)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let painter = ui.painter();
            let scrim = egui::Color32::from_black_alpha(170);
            if let Some(t) = target {
                // Four rects around the cutout.
                painter.rect_filled(
                    egui::Rect::from_min_max(screen.min, egui::pos2(screen.max.x, t.min.y)),
                    0.0,
                    scrim,
                );
                painter.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(screen.min.x, t.max.y), screen.max),
                    0.0,
                    scrim,
                );
                painter.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(screen.min.x, t.min.y), egui::pos2(t.min.x, t.max.y)),
                    0.0,
                    scrim,
                );
                painter.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(t.max.x, t.min.y), egui::pos2(screen.max.x, t.max.y)),
                    0.0,
                    scrim,
                );
                // Pulsing accent ring.
                let phase = (ctx.input(|i| i.time).sin() as f32 * 0.5 + 0.5) * 0.55 + 0.45;
                let ring = egui::Color32::from_rgba_unmultiplied(
                    ACCENT.r(),
                    ACCENT.g(),
                    ACCENT.b(),
                    (255.0 * phase) as u8,
                );
                painter.rect_stroke(t, 8.0, egui::Stroke::new(2.5, ring), egui::StrokeKind::Outside);
                ctx.request_repaint();
            } else {
                painter.rect_filled(screen, 0.0, scrim);
            }
        });

    if caption.is_empty() {
        return;
    }
    // Caption callout: just below the target (or centred when no target).
    let card_w = 300.0;
    let pos = match target {
        Some(t) => {
            let x = (t.center().x - card_w * 0.5).clamp(screen.left() + 12.0, screen.right() - card_w - 12.0);
            let below = t.max.y + 14.0;
            let y = if below + 70.0 <= screen.bottom() { below } else { (t.min.y - 84.0).max(screen.top() + 12.0) };
            egui::pos2(x, y)
        }
        None => egui::pos2(screen.center().x - card_w * 0.5, screen.center().y - 40.0),
    };
    egui::Area::new(egui::Id::new("lunco_spotlight_caption"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            ui.set_width(card_w);
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(20, 28, 44, 250))
                .corner_radius(12.0)
                .stroke(egui::Stroke::new(1.5, ACCENT))
                .inner_margin(egui::Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(&caption)
                            .color(egui::Color32::from_rgb(214, 228, 250))
                            .size(15.0),
                    );
                });
        });
}

/// Adds the [`TutorialHud`] resource, its four commands, and the two ui-gated
/// overlay draw systems (ordered after [`WorkbenchRenderSet`](crate::WorkbenchRenderSet)
/// so panel `HelpAnchors` rects are populated before the spotlight reads them).
/// Idempotent. Registered by [`WorkbenchPlugin`](crate::WorkbenchPlugin).
pub struct TutorialOverlayPlugin;

impl Plugin for TutorialOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TutorialHud>();
        register_all_commands(app);
        app.add_systems(
            EguiPrimaryContextPass,
            (draw_tutorial_hud, draw_spotlight).after(crate::WorkbenchRenderSet),
        );
    }
}
