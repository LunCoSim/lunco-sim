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

/// Side of the spotlight target the coach card sits on — drives where the
/// speech-bubble tail's apex points.
#[derive(Clone, Copy)]
enum CalloutSide {
    Right,
    Below,
    Above,
    Left,
    /// Card sits *on* the target (huge central panels / no room alongside) — no
    /// tail.
    Over,
    /// No target — centred, no tail.
    Centred,
}

/// Fire a tour-navigation event on the bus. The data driver (and rhai tours)
/// advance on these; `data` carries the jump index for `cmd:TutorialGoto` and
/// the pin flag for `cmd:TutorialPin`.
fn emit_tour(commands: &mut Commands, name: &str, data: lunco_core::TelemetryValue) {
    commands.trigger(lunco_core::TelemetryEvent {
        name: name.to_string(),
        source: 0,
        severity: lunco_core::Severity::Info,
        data,
        timestamp: 0.0,
    });
}

/// Draw the guided-tour coach mark: scrim + pulsing ring on the step's anchor,
/// a speech-bubble tail, and a themed card with a full-width accent banner,
/// body, progress bar, clickable jump-dots, and Back / Skip / Next·Done
/// controls (plus a "show on next start" toggle for data tours). Controls fire
/// `cmd:Tutorial{Next,Back,Skip,Goto,Pin}` on the bus; the driver advances on
/// them. Matches the lunica product tour's polish so both apps share one card.
fn draw_tour(
    mut egui_ctx: EguiContexts,
    hud: Res<TutorialHud>,
    anchors: Res<crate::HelpAnchors>,
    theme: Option<Res<lunco_theme::Theme>>,
    active: Option<Res<crate::tour_driver::ActiveTour>>,
    seen: Option<Res<crate::tour_driver::TourSeen>>,
    mut commands: Commands,
) {
    let Some(step) = hud.tour.clone() else { return };
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let screen = ctx.content_rect();

    let theme = theme.map(|t| t.clone()).unwrap_or_else(lunco_theme::Theme::dark);
    let accent = theme.tokens.accent;
    let accent_text = theme.colors.base;
    let muted = theme.tokens.text_subdued;
    let text = theme.colors.text;
    let card_fill = {
        let [r, g, b, _] = theme.tokens.surface_raised.to_array();
        egui::Color32::from_rgba_unmultiplied(r, g, b, 250)
    };

    // "Show on next start" checkbox — only meaningful while a *data* tour is
    // active (rhai tours don't track a persisted seen-set).
    let (show_checkbox, seen_now) = match (active.as_ref().and_then(|a| a.id), seen.as_ref()) {
        (Some(id), Some(seen)) => (true, seen.seen.iter().any(|s| s == id.as_str())),
        _ => (false, false),
    };

    let target = if step.anchor.is_empty() {
        None
    } else {
        anchors
            .get(&step.anchor)
            .map(|r| r.expand(6.0).intersect(screen))
            .filter(|r| r.width() > 4.0 && r.height() > 4.0)
    };

    // ── Card placement — pick the side that fits around the target, matching
    // the lunica tour's Right/Below/Above/Left/Over/Centred logic.
    let card_w = 360.0;
    let card_h_est = 300.0;
    let margin = 18.0;
    let (side, card_pos) = if let Some(t) = target {
        let over_pos = egui::pos2(
            (t.center().x - card_w * 0.5)
                .clamp(screen.min.x + margin, screen.max.x - card_w - margin),
            (t.min.y + 16.0).clamp(screen.min.y + margin, screen.max.y - card_h_est - margin),
        );
        let target_huge =
            t.width() > screen.width() * 0.55 && t.height() > screen.height() * 0.5;
        let target_short = t.height() < 50.0;
        let below_y = if target_short {
            (t.max.y + 80.0).clamp(screen.min.y + margin, screen.max.y - card_h_est - margin)
        } else {
            t.max.y + margin
        };
        let candidates = [
            (
                CalloutSide::Right,
                egui::pos2(
                    t.max.x + margin,
                    (t.center().y - card_h_est * 0.5)
                        .clamp(screen.min.y + margin, screen.max.y - card_h_est - margin),
                ),
            ),
            (
                CalloutSide::Below,
                egui::pos2(
                    (t.center().x - card_w * 0.5)
                        .clamp(screen.min.x + margin, screen.max.x - card_w - margin),
                    below_y,
                ),
            ),
            (
                CalloutSide::Above,
                egui::pos2(
                    (t.center().x - card_w * 0.5)
                        .clamp(screen.min.x + margin, screen.max.x - card_w - margin),
                    t.min.y - card_h_est - margin,
                ),
            ),
            (
                CalloutSide::Left,
                egui::pos2(
                    t.min.x - card_w - margin,
                    (t.center().y - card_h_est * 0.5)
                        .clamp(screen.min.y + margin, screen.max.y - card_h_est - margin),
                ),
            ),
        ];
        let fits = |p: &egui::Pos2| {
            p.x >= screen.min.x + margin
                && p.x + card_w <= screen.max.x - margin
                && p.y >= screen.min.y + margin
                && p.y + card_h_est <= screen.max.y - margin
        };
        if target_huge {
            (CalloutSide::Over, over_pos)
        } else {
            candidates
                .into_iter()
                .find(|(_, p)| fits(p))
                .unwrap_or((CalloutSide::Over, over_pos))
        }
    } else {
        (
            CalloutSide::Centred,
            egui::pos2(
                screen.center().x - card_w * 0.5,
                screen.center().y - card_h_est * 0.5,
            ),
        )
    };

    // ── Scrim + ring + speech-bubble tail (behind the card) ──────────────────
    egui::Area::new(egui::Id::new("lunco_tour_scrim"))
        .order(egui::Order::Foreground)
        .interactable(false)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let painter = ui.painter();
            paint_scrim(painter, ctx, screen, target);
            if let Some(t) = target {
                let card_rect =
                    egui::Rect::from_min_size(card_pos, egui::vec2(card_w, card_h_est));
                if let Some((apex, b1, b2)) = tour_tail_points(side, t, card_rect) {
                    painter.add(egui::Shape::Path(egui::epaint::PathShape {
                        points: vec![apex, b1, b2],
                        closed: true,
                        fill: card_fill,
                        stroke: egui::Stroke::new(1.0, accent.linear_multiply(0.55)).into(),
                    }));
                }
            }
        });

    // ── Card ─────────────────────────────────────────────────────────────────
    let last = step.index + 1 >= step.total;
    let mut next = false;
    let mut back = false;
    let mut skip = false;
    let mut goto: Option<usize> = None;
    let mut pin: Option<bool> = None;

    egui::Area::new(egui::Id::new("lunco_tour_card"))
        .order(egui::Order::Tooltip)
        .interactable(true)
        .fixed_pos(card_pos)
        .show(ctx, |ui| {
            ui.set_width(card_w);
            egui::Frame::new()
                .fill(card_fill)
                .corner_radius(14.0)
                .inner_margin(egui::Margin::ZERO)
                .stroke(egui::Stroke::new(1.5, accent))
                .show(ui, |ui| {
                    // Banner — full-width accent stripe with diagonal pinstripes.
                    let banner_h = 32.0;
                    let (banner_rect, _) =
                        ui.allocate_exact_size(egui::vec2(card_w, banner_h), egui::Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(
                        banner_rect,
                        egui::CornerRadius { nw: 13, ne: 13, sw: 0, se: 0 },
                        accent,
                    );
                    let stripe = accent_text.linear_multiply(0.12);
                    let mut x = banner_rect.min.x - banner_h;
                    while x < banner_rect.max.x {
                        p.line_segment(
                            [
                                egui::pos2(x, banner_rect.max.y),
                                egui::pos2(x + banner_h, banner_rect.min.y),
                            ],
                            egui::Stroke::new(1.5, stripe),
                        );
                        x += 10.0;
                    }
                    let banner_label = if step.title.is_empty() {
                        "🎓  INTERACTIVE TUTORIAL".to_string()
                    } else {
                        format!("🎓  {}", step.title.to_uppercase())
                    };
                    p.text(
                        banner_rect.min + egui::vec2(14.0, banner_h * 0.5),
                        egui::Align2::LEFT_CENTER,
                        banner_label,
                        egui::FontId::proportional(12.5),
                        accent_text,
                    );
                    if step.total > 0 {
                        p.text(
                            banner_rect.max - egui::vec2(14.0, banner_h * 0.5),
                            egui::Align2::RIGHT_CENTER,
                            format!("Step {} / {}", step.index + 1, step.total),
                            egui::FontId::proportional(11.5),
                            accent_text,
                        );
                    }

                    ui.add_space(2.0);
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(18, 14))
                        .show(ui, |ui| {
                            if !step.body.is_empty() {
                                ui.label(egui::RichText::new(&step.body).size(14.0).color(text));
                                ui.add_space(10.0);
                            }

                            // Progress bar.
                            if step.total > 0 {
                                let (bar, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), 4.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(bar, 2.0, muted.linear_multiply(0.25));
                                let frac = (step.index as f32 + 1.0) / step.total as f32;
                                let fill = egui::Rect::from_min_max(
                                    bar.min,
                                    egui::pos2(bar.min.x + bar.width() * frac, bar.max.y),
                                );
                                ui.painter().rect_filled(fill, 2.0, accent);
                                ui.add_space(8.0);

                                // Clickable jump-dots.
                                ui.horizontal_wrapped(|ui| {
                                    for i in 0..step.total {
                                        let is_cur = i == step.index;
                                        let done = i < step.index;
                                        let color = if is_cur {
                                            accent
                                        } else if done {
                                            accent.linear_multiply(0.5)
                                        } else {
                                            muted.linear_multiply(0.4)
                                        };
                                        let (dot, resp) = ui.allocate_exact_size(
                                            egui::vec2(14.0, 14.0),
                                            egui::Sense::click(),
                                        );
                                        ui.painter().circle_filled(
                                            dot.center(),
                                            if is_cur { 5.0 } else { 3.5 },
                                            color,
                                        );
                                        if resp.clicked() {
                                            goto = Some(i);
                                        }
                                        resp.on_hover_text(format!("Step {}", i + 1));
                                    }
                                });
                                ui.add_space(10.0);
                            }

                            // Buttons.
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(
                                        step.index > 0,
                                        egui::Button::new("◀  Back")
                                            .min_size(egui::vec2(80.0, 28.0)),
                                    )
                                    .clicked()
                                {
                                    back = true;
                                }
                                if ui
                                    .button(egui::RichText::new("Skip").color(muted).size(11.0))
                                    .clicked()
                                {
                                    skip = true;
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let label = if last { "Done ✓" } else { "Next  ▶" };
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(label)
                                                        .strong()
                                                        .color(accent_text),
                                                )
                                                .fill(accent)
                                                .min_size(egui::vec2(90.0, 28.0)),
                                            )
                                            .clicked()
                                        {
                                            next = true;
                                        }
                                    },
                                );
                            });

                            if show_checkbox {
                                ui.add_space(4.0);
                                let mut show_next = !seen_now;
                                if ui
                                    .checkbox(&mut show_next, "Show on next start")
                                    .on_hover_text(
                                        "Re-open this tour automatically next time you \
                                         launch the app.",
                                    )
                                    .changed()
                                {
                                    pin = Some(show_next);
                                }
                            }
                        });
                });
        });

    if next {
        emit_tour(&mut commands, "cmd:TutorialNext", lunco_core::TelemetryValue::Bool(true));
    }
    if back {
        emit_tour(&mut commands, "cmd:TutorialBack", lunco_core::TelemetryValue::Bool(true));
    }
    if skip {
        emit_tour(&mut commands, "cmd:TutorialSkip", lunco_core::TelemetryValue::Bool(true));
    }
    if let Some(i) = goto {
        emit_tour(&mut commands, "cmd:TutorialGoto", lunco_core::TelemetryValue::I64(i as i64));
    }
    if let Some(b) = pin {
        emit_tour(&mut commands, "cmd:TutorialPin", lunco_core::TelemetryValue::Bool(b));
    }
}

/// Three points of the speech-bubble tail triangle: apex (near the target) and
/// two base points on the card edge. `None` for `Over`/`Centred` (no tail).
fn tour_tail_points(
    side: CalloutSide,
    target: egui::Rect,
    card: egui::Rect,
) -> Option<(egui::Pos2, egui::Pos2, egui::Pos2)> {
    let base_half = 10.0;
    Some(match side {
        CalloutSide::Right => {
            let edge_x = card.min.x;
            let cy = target
                .center()
                .y
                .clamp(card.min.y + base_half + 4.0, card.max.y - base_half - 4.0);
            (
                egui::pos2(target.max.x, cy),
                egui::pos2(edge_x + 0.5, cy - base_half),
                egui::pos2(edge_x + 0.5, cy + base_half),
            )
        }
        CalloutSide::Left => {
            let edge_x = card.max.x;
            let cy = target
                .center()
                .y
                .clamp(card.min.y + base_half + 4.0, card.max.y - base_half - 4.0);
            (
                egui::pos2(target.min.x, cy),
                egui::pos2(edge_x - 0.5, cy - base_half),
                egui::pos2(edge_x - 0.5, cy + base_half),
            )
        }
        CalloutSide::Below => {
            let edge_y = card.min.y;
            let cx = target
                .center()
                .x
                .clamp(card.min.x + base_half + 4.0, card.max.x - base_half - 4.0);
            (
                egui::pos2(cx, target.max.y),
                egui::pos2(cx - base_half, edge_y + 0.5),
                egui::pos2(cx + base_half, edge_y + 0.5),
            )
        }
        CalloutSide::Above => {
            let edge_y = card.max.y;
            let cx = target
                .center()
                .x
                .clamp(card.min.x + base_half + 4.0, card.max.x - base_half - 4.0);
            (
                egui::pos2(cx, target.min.y),
                egui::pos2(cx - base_half, edge_y - 0.5),
                egui::pos2(cx + base_half, edge_y - 0.5),
            )
        }
        CalloutSide::Over | CalloutSide::Centred => return None,
    })
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
