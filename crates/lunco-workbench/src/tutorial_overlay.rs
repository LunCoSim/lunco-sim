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
    /// Active guided-tour coach step (lunica-style). When set, the overlay draws
    /// the scrim+ring on `anchor` plus a coach card with a banner, body, progress
    /// dots, and Back/Next/Skip controls. Takes over the scrim from `spotlight`.
    /// `None` = no tour. Driven from rhai via `coach(...)` / `end_tour()`.
    pub tour: Option<TourStep>,
}

/// One coach-mark step of a guided tour (see [`TutorialHud::tour`]).
#[derive(Clone, Debug, Default)]
pub struct TourStep {
    /// 0-based step index (drives the progress dots).
    pub index: usize,
    /// Total step count (drives the progress dots + Next→Done label).
    pub total: usize,
    /// `HelpAnchors` key to spotlight; empty/unknown = centred card, no cutout.
    pub anchor: String,
    /// Card banner title.
    pub title: String,
    /// Card body text.
    pub body: String,
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

/// Show a guided-tour coach step: spotlight `anchor`, and draw a coach card with
/// `title`/`body`, progress dots (`index`/`total`), and Back/Next/Skip controls.
/// Rhai: `coach(index, total, anchor, title, body)`. The controls emit
/// `cmd:TutorialNext` / `cmd:TutorialBack` / `cmd:TutorialSkip` on the event bus,
/// which the tour script advances on (a script can simulate a click with
/// `emit("cmd:TutorialNext", 0)`).
#[Command(default)]
pub struct SetTourStep {
    /// 0-based step index (progress dots).
    pub index: i64,
    /// Total step count.
    pub total: i64,
    /// `HelpAnchors` key to spotlight; empty = centred card.
    #[serde(default)]
    #[reflect(default)]
    pub anchor: String,
    /// Coach-card banner title.
    #[serde(default)]
    #[reflect(default)]
    pub title: String,
    /// Coach-card body text.
    #[serde(default)]
    #[reflect(default)]
    pub body: String,
}

/// End the guided tour (hide the coach card + scrim). Rhai: `end_tour()`.
#[Command(default)]
pub struct ClearTour {}

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

#[on_command(SetTourStep)]
fn on_set_tour_step(cmd: SetTourStep, mut hud: ResMut<TutorialHud>) {
    hud.tour = Some(TourStep {
        index: cmd.index.max(0) as usize,
        total: cmd.total.max(0) as usize,
        anchor: cmd.anchor.clone(),
        title: cmd.title.clone(),
        body: cmd.body.clone(),
    });
}

#[on_command(ClearTour)]
fn on_clear_tour(_cmd: ClearTour, mut hud: ResMut<TutorialHud>) {
    hud.tour = None;
}

register_commands!(
    on_set_hint,
    on_set_objectives,
    on_spotlight,
    on_clear_spotlight,
    on_set_tour_step,
    on_clear_tour,
);

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
    // A guided tour owns the scrim (see `draw_tour`); don't double-dim.
    if hud.tour.is_some() {
        return;
    }
    let Some((key, caption)) = hud.spotlight.clone() else { return };
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let screen = ctx.content_rect();
    let target = anchors.get(&key);

    egui::Area::new(egui::Id::new("lunco_spotlight_scrim"))
        .order(egui::Order::Foreground)
        .interactable(false)
        .fixed_pos(screen.min)
        .show(ctx, |ui| paint_scrim(ui.painter(), ctx, screen, target));

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

/// Dim everything except `target` (four rects + pulsing accent ring), or full-dim
/// when `target` is `None`. Shared by the spotlight and the guided-tour coach.
fn paint_scrim(
    painter: &egui::Painter,
    ctx: &egui::Context,
    screen: egui::Rect,
    target: Option<egui::Rect>,
) {
    let scrim = egui::Color32::from_black_alpha(170);
    let Some(t) = target else {
        painter.rect_filled(screen, 0.0, scrim);
        return;
    };
    painter.rect_filled(egui::Rect::from_min_max(screen.min, egui::pos2(screen.max.x, t.min.y)), 0.0, scrim);
    painter.rect_filled(egui::Rect::from_min_max(egui::pos2(screen.min.x, t.max.y), screen.max), 0.0, scrim);
    painter.rect_filled(egui::Rect::from_min_max(egui::pos2(screen.min.x, t.min.y), egui::pos2(t.min.x, t.max.y)), 0.0, scrim);
    painter.rect_filled(egui::Rect::from_min_max(egui::pos2(t.max.x, t.min.y), egui::pos2(screen.max.x, t.max.y)), 0.0, scrim);
    let phase = (ctx.input(|i| i.time).sin() as f32 * 0.5 + 0.5) * 0.55 + 0.45;
    let ring = egui::Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), (255.0 * phase) as u8);
    painter.rect_stroke(t, 8.0, egui::Stroke::new(2.5, ring), egui::StrokeKind::Outside);
    ctx.request_repaint();
}

/// Draw the guided-tour coach mark: scrim+ring on the step's anchor, plus a card
/// with a "🎓 TUTORIAL" banner, title, body, progress dots, and Back/Next/Skip
/// controls. The controls fire `cmd:TutorialBack`/`cmd:TutorialNext`/
/// `cmd:TutorialSkip` on the TelemetryEvent bus; the tour script advances on them.
fn draw_tour(
    mut egui_ctx: EguiContexts,
    hud: Res<TutorialHud>,
    anchors: Res<crate::HelpAnchors>,
    mut commands: Commands,
) {
    let Some(step) = hud.tour.clone() else { return };
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let screen = ctx.content_rect();
    let target = if step.anchor.is_empty() { None } else { anchors.get(&step.anchor) };

    egui::Area::new(egui::Id::new("lunco_tour_scrim"))
        .order(egui::Order::Foreground)
        .interactable(false)
        .fixed_pos(screen.min)
        .show(ctx, |ui| paint_scrim(ui.painter(), ctx, screen, target));

    // Card placement: below the target, else centred.
    let card_w = 340.0;
    let pos = match target {
        Some(t) => {
            let x = (t.center().x - card_w * 0.5).clamp(screen.left() + 12.0, screen.right() - card_w - 12.0);
            let below = t.max.y + 16.0;
            let y = if below + 120.0 <= screen.bottom() { below } else { (t.min.y - 150.0).max(screen.top() + 12.0) };
            egui::pos2(x, y)
        }
        None => egui::pos2(screen.center().x - card_w * 0.5, screen.center().y - 70.0),
    };

    let mut nav: Option<&str> = None;
    egui::Area::new(egui::Id::new("lunco_tour_card"))
        .order(egui::Order::Tooltip)
        .interactable(true)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            ui.set_width(card_w);
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(20, 28, 44, 252))
                .corner_radius(14.0)
                .stroke(egui::Stroke::new(1.5, ACCENT))
                .inner_margin(egui::Margin::symmetric(16, 14))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("🎓  TUTORIAL").color(ACCENT).small().strong());
                    if !step.title.is_empty() {
                        ui.add_space(3.0);
                        ui.label(egui::RichText::new(&step.title).color(egui::Color32::from_rgb(230, 240, 255)).size(17.0).strong());
                    }
                    if !step.body.is_empty() {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(&step.body).color(egui::Color32::from_rgb(206, 220, 244)).size(15.0));
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        // Progress dots.
                        for i in 0..step.total {
                            let filled = i == step.index;
                            ui.label(
                                egui::RichText::new(if filled { "●" } else { "○" })
                                    .color(if filled { ACCENT } else { egui::Color32::from_gray(110) })
                                    .size(12.0),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let last = step.index + 1 >= step.total;
                            if ui.button(if last { "Done" } else { "Next ▶" }).clicked() {
                                nav = Some("cmd:TutorialNext");
                            }
                            if step.index > 0 && ui.button("◀ Back").clicked() {
                                nav = Some("cmd:TutorialBack");
                            }
                            if !last && ui.button("Skip").clicked() {
                                nav = Some("cmd:TutorialSkip");
                            }
                        });
                    });
                });
        });

    if let Some(name) = nav {
        commands.trigger(lunco_core::TelemetryEvent {
            name: name.to_string(),
            source: 0,
            severity: lunco_core::Severity::Info,
            data: lunco_core::TelemetryValue::Bool(true),
            timestamp: 0.0,
        });
    }
}

/// Adds the [`TutorialHud`] resource, its commands, and the ui-gated overlay draw
/// systems (ordered after [`WorkbenchRenderSet`](crate::WorkbenchRenderSet) so
/// panel `HelpAnchors` rects are populated before the spotlight/tour read them).
/// Idempotent. Registered by [`WorkbenchPlugin`](crate::WorkbenchPlugin).
pub struct TutorialOverlayPlugin;

impl Plugin for TutorialOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TutorialHud>();
        register_all_commands(app);
        app.add_systems(
            EguiPrimaryContextPass,
            (draw_tutorial_hud, draw_spotlight, draw_tour).after(crate::WorkbenchRenderSet),
        );
    }
}
