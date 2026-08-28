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
//! luncosim and the lunica Modelica workbench load `WorkbenchPlugin`, so the
//! same HUD is available to every app. The [`HelpAnchors`](crate::HelpAnchors)
//! rect registry it spotlights against already lives here too.
//!
//! Command payloads are single strings (objectives arrive pre-formatted as a
//! checklist block from the rhai prelude) — the same trivially-marshalled shape
//! as `ShowNotification.text`, avoiding any nested-collection reflection.

use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use lunco_core::{on_command, register_commands, Command};

/// Persistent tutorial HUD + spotlight state. Always present (headless too) so
/// the commands never panic on a missing resource; only the draw is ui-gated.
#[derive(Resource, Default, Clone, Debug)]
pub struct TutorialHud {
    /// Display title of the currently running tutorial. Empty when no lesson
    /// owns the HUD; the workbench status bar uses this as the primary lesson
    /// identity and keeps the loaded USD filename as the secondary identity.
    pub title: String,
    /// One-line instruction shown at the top of the HUD card. Empty = hidden.
    pub hint: String,
    /// Pre-formatted objectives checklist block (one objective per line, with a
    /// leading glyph). Empty = the objectives card is hidden.
    pub objectives: String,
    /// Active spotlight: `(anchor_key, caption)`. `anchor_key` resolves against
    /// [`HelpAnchors`](crate::HelpAnchors); a named key must resolve to a
    /// visible widget. `None` = no spotlight.
    pub spotlight: Option<(String, String)>,
    /// Active guided-tour coach step (lunica-style). When set, the overlay draws
    /// the scrim+ring on `anchor` plus a coach card with a banner, body, progress
    /// dots, and Back/Next/Skip controls. Takes over the scrim from `spotlight`.
    /// `None` = no tour. Driven from rhai via `coach(...)` / `end_tour()`.
    pub tour: Option<TourStep>,
    /// Last named anchor reported as unavailable. Prevents one render pass
    /// from publishing the same authored UI contract failure every frame.
    pub reported_missing_anchor: Option<String>,
    /// Recoverable presentation error shown above the lesson instead of
    /// tearing down its host or the loaded world.
    pub recovery: Option<TutorialRecovery>,
}

/// A tutorial presentation problem that does not invalidate the simulation.
/// The owning tutorial decides what Continue means; the workbench only presents
/// the recovery action and emits the typed request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TutorialRecovery {
    /// Authored help-anchor key that could not be resolved.
    pub anchor: String,
    /// Human-readable explanation displayed in the recovery surface.
    pub detail: String,
}

/// The authored tutorial target could not be resolved in the current workbench
/// presentation. The tutorial launcher owns the lifecycle response; the overlay
/// only reports this generic UI contract failure.
#[derive(Event, Clone, Debug)]
pub struct TutorialTargetUnavailable {
    /// The authored [`HelpAnchors`](crate::HelpAnchors) key that was absent.
    pub anchor: String,
}

/// One coach-mark step of a guided tour (see [`TutorialHud::tour`]).
#[derive(Clone, Debug, Default)]
pub struct TourStep {
    /// 0-based step index (drives the progress dots).
    pub index: usize,
    /// Total step count (drives the progress dots + Next→Done label).
    pub total: usize,
    /// `HelpAnchors` key to spotlight; empty = centred card, a named key must
    /// resolve to a visible widget.
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

/// Stop the active tutorial from the coach card itself. The tutorial crate
/// owns the lifecycle command; this event keeps the overlay independent of
/// that crate while making Stop available wherever the coach card appears.
#[derive(Event, Clone, Copy, Debug, Default)]
pub struct TutorialStopRequested;

/// Continue the current lesson after dismissing a recoverable presentation
/// problem. The tutorial launcher advances a guided step when one is active.
#[derive(Event, Clone, Copy, Debug, Default)]
pub struct TutorialRecoveryContinueRequested;

/// Retry resolving the current authored presentation target.
#[derive(Event, Clone, Copy, Debug, Default)]
pub struct TutorialRecoveryRetryRequested;

/// Advance a guided tutorial step through the shared typed-command bus.
/// The command projector supplies the established `cmd:TutorialNext` event
/// consumed by authored Rhai tours.
#[Command(default)]
pub struct TutorialNext {}

/// Return to the previous guided tutorial step.
#[Command(default)]
pub struct TutorialBack {}

/// Stop the current guided tutorial tour.
#[Command(default)]
pub struct TutorialSkip {}

fn on_tutorial_next(_trigger: On<TutorialNext>, mut world: DeferredWorld) {
    world.trigger(lunco_core::command_telemetry_event(stringify!(
        TutorialNext
    )));
}

fn on_tutorial_back(_trigger: On<TutorialBack>, mut world: DeferredWorld) {
    world.trigger(lunco_core::command_telemetry_event(stringify!(
        TutorialBack
    )));
}

fn on_tutorial_skip(_trigger: On<TutorialSkip>, mut world: DeferredWorld) {
    world.trigger(lunco_core::command_telemetry_event(stringify!(
        TutorialSkip
    )));
}

#[on_command(SetHint)]
fn on_set_hint(trigger: On<SetHint>, mut hud: ResMut<TutorialHud>) {
    hud.hint = cmd.text.clone();
}

#[on_command(SetObjectives)]
fn on_set_objectives(trigger: On<SetObjectives>, mut hud: ResMut<TutorialHud>) {
    hud.objectives = cmd.text.clone();
}

#[on_command(Spotlight)]
fn on_spotlight(trigger: On<Spotlight>, mut hud: ResMut<TutorialHud>) {
    hud.spotlight = Some((cmd.anchor.clone(), cmd.text.clone()));
    hud.reported_missing_anchor = None;
    hud.recovery = None;
}

#[on_command(ClearSpotlight)]
fn on_clear_spotlight(trigger: On<ClearSpotlight>, mut hud: ResMut<TutorialHud>) {
    hud.spotlight = None;
    hud.reported_missing_anchor = None;
    hud.recovery = None;
}

#[on_command(SetTourStep)]
fn on_set_tour_step(trigger: On<SetTourStep>, mut hud: ResMut<TutorialHud>) {
    hud.tour = Some(TourStep {
        index: cmd.index.max(0) as usize,
        total: cmd.total.max(0) as usize,
        anchor: cmd.anchor.clone(),
        title: cmd.title.clone(),
        body: cmd.body.clone(),
    });
    hud.reported_missing_anchor = None;
    hud.recovery = None;
}

#[on_command(ClearTour)]
fn on_clear_tour(trigger: On<ClearTour>, mut hud: ResMut<TutorialHud>) {
    hud.tour = None;
    hud.reported_missing_anchor = None;
    hud.recovery = None;
}

register_commands!(
    on_set_hint,
    on_set_objectives,
    on_spotlight,
    on_clear_spotlight,
    on_set_tour_step,
    on_clear_tour,
);

fn register_tutorial_navigation(app: &mut App) {
    app.register_type::<TutorialNext>()
        .register_type::<TutorialBack>()
        .register_type::<TutorialSkip>()
        .add_observer(on_tutorial_next)
        .add_observer(on_tutorial_back)
        .add_observer(on_tutorial_skip);
}

// ── Rendering ─────────────────────────────────────────────────────────────

/// Draw the persistent objectives/hint card, top-left, below the menu bar.
/// Non-interactive and in the foreground layer so it stays visible across
/// perspectives without eating clicks. Tooltip-order popups and the menu bar
/// remain higher in the egui hierarchy.
fn draw_tutorial_hud(
    mut egui_ctx: EguiContexts,
    hud: Res<TutorialHud>,
    theme: Option<Res<lunco_theme::Theme>>,
) {
    if hud.hint.is_empty() && hud.objectives.is_empty() {
        return;
    }
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    // The viewport rect is the full egui viewport. The foreground layer keeps
    // this non-interactive HUD above ordinary panels while popup surfaces can
    // still take precedence.
    let screen = ctx.viewport_rect();
    let theme = theme
        .map(|t| t.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);
    let accent = theme.tokens.accent;

    egui::Area::new(egui::Id::new("lunco_tutorial_hud"))
        .order(egui::Order::Foreground)
        .interactable(false)
        .fixed_pos(egui::pos2(screen.left() + 16.0, screen.top() + 44.0))
        .show(ctx, |ui| {
            ui.set_max_width(320.0);
            egui::Frame::new()
                .fill(theme.tokens.surface_raised)
                .corner_radius(10.0)
                .stroke(egui::Stroke::new(1.0, accent.linear_multiply(0.6)))
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    if !hud.objectives.is_empty() {
                        ui.label(
                            egui::RichText::new("OBJECTIVES")
                                .color(accent)
                                .small()
                                .strong(),
                        );
                        ui.add_space(2.0);
                        for line in hud.objectives.lines() {
                            // Colour done/failed lines by their leading glyph.
                            let color = match line.chars().next() {
                                Some('✓') => theme.tokens.success,
                                Some('✗') => theme.tokens.error,
                                Some('▸') => theme.tokens.text,
                                _ => theme.tokens.text_subdued,
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
                                .color(theme.tokens.text)
                                .size(15.0),
                        );
                    }
                });
        });
}

/// Draw a blocking but non-terminal recovery surface for an unavailable
/// authored tutorial target. It owns input until the learner chooses an action,
/// while leaving the lesson host, simulation, and loaded scene alive.
fn draw_tutorial_recovery(
    mut egui_ctx: EguiContexts,
    hud: Res<TutorialHud>,
    theme: Option<Res<lunco_theme::Theme>>,
    mut commands: Commands,
) {
    let Some(recovery) = hud.recovery.clone() else {
        return;
    };
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let screen = ctx.viewport_rect();
    let theme = theme
        .map(|t| t.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);
    let card_fill = {
        let [r, g, b, _] = theme.tokens.surface_raised.to_array();
        egui::Color32::from_rgba_unmultiplied(r, g, b, 252)
    };
    let card_w = 480.0_f32.min(screen.width() - 32.0).max(280.0);
    let card_pos = egui::pos2(
        (screen.center().x - card_w * 0.5).max(screen.left() + 16.0),
        (screen.center().y - 150.0).max(screen.top() + 48.0),
    );

    // This scrim is interactive so clicks cannot leak into the scene or the
    // underlying workbench while the learner chooses a recovery action.
    egui::Area::new(egui::Id::new("lunco_tutorial_recovery_scrim"))
        .order(egui::Order::Tooltip)
        .interactable(true)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
            ui.painter()
                .rect_filled(rect, 0.0, theme.tokens.scrim.linear_multiply(0.84));
        });

    let mut continue_lesson = false;
    let mut retry = false;
    let mut stop = false;
    egui::Area::new(egui::Id::new("lunco_tutorial_recovery_card"))
        .order(egui::Order::Tooltip)
        .interactable(true)
        .fixed_pos(card_pos)
        .show(ctx, |ui| {
            ui.set_width(card_w);
            egui::Frame::new()
                .fill(card_fill)
                .corner_radius(12.0)
                .stroke(egui::Stroke::new(1.5, theme.tokens.warning))
                .inner_margin(egui::Margin::symmetric(20, 16))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (icon_rect, _) = ui.allocate_exact_size(
                            egui::vec2(28.0, 28.0),
                            egui::Sense::hover(),
                        );
                        crate::paint_icon(
                            ui.painter(),
                            crate::UiIcon::Warning,
                            icon_rect,
                            theme.tokens.warning,
                        );
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("Tutorial view unavailable")
                                    .strong()
                                    .size(17.0),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "The lesson is still running; only this presentation target is missing.",
                                )
                                .color(theme.tokens.text_subdued)
                                .small(),
                            );
                        });
                    });
                    ui.add_space(10.0);
                    ui.label(&recovery.detail);
                    ui.label(
                        egui::RichText::new(format!("Authored target: {}", recovery.anchor))
                            .color(theme.tokens.text_subdued)
                            .small(),
                    );
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if crate::icon_text_button(
                            ui,
                            crate::UiIcon::Forward,
                            "Continue",
                            "Continue the lesson without this view",
                        )
                        .clicked()
                        {
                            continue_lesson = true;
                        }
                        if crate::icon_text_button(
                            ui,
                            crate::UiIcon::Refresh,
                            "Retry",
                            "Try resolving the authored view again",
                        )
                        .clicked()
                        {
                            retry = true;
                        }
                        if crate::icon_text_button(
                            ui,
                            crate::UiIcon::Stop,
                            "Stop",
                            "Stop the tutorial and clear its owned scene",
                        )
                        .clicked()
                        {
                            stop = true;
                        }
                    });
                });
        });

    if continue_lesson {
        commands.trigger(TutorialRecoveryContinueRequested);
    }
    if retry {
        commands.trigger(TutorialRecoveryRetryRequested);
    }
    if stop {
        commands.trigger(TutorialStopRequested);
    }
}

/// Derive the tutorial content region from the workbench's published menu-bar
/// geometry. The menu owns its height; the tutorial overlay must not duplicate
/// that layout constant or drift when the chrome changes.
fn tutorial_content_rect(ctx: &egui::Context, anchors: &crate::HelpAnchors) -> Option<egui::Rect> {
    let viewport = ctx.viewport_rect();
    let menu = anchors.get("menu.bar")?;
    Some(egui::Rect::from_min_max(
        egui::pos2(viewport.left(), menu.bottom()),
        viewport.max,
    ))
}

fn tutorial_anchor_rect(
    anchors: &crate::HelpAnchors,
    key: &str,
    content: egui::Rect,
) -> Option<egui::Rect> {
    anchors
        .get(key)
        .map(|rect| {
            let rect = rect.expand(6.0);
            // Title-bar/menu anchors are deliberately outside the workbench
            // content region. All other anchors must belong to the visible
            // content so a stale panel rect cannot become a false target.
            if key.starts_with("menu.") {
                rect
            } else {
                rect.intersect(content)
            }
        })
        .filter(|rect| rect.width() > 4.0 && rect.height() > 4.0)
}

fn report_missing_anchor(hud: &mut TutorialHud, commands: &mut Commands, anchor: &str) {
    if hud.reported_missing_anchor.as_deref() == Some(anchor) {
        return;
    }
    hud.reported_missing_anchor = Some(anchor.to_owned());
    commands.trigger(TutorialTargetUnavailable {
        anchor: anchor.to_owned(),
    });
}

/// Draw the spotlight: dim the screen except the anchored widget's rect, ring
/// it with a pulsing accent, and show a caption callout. A named anchor that is
/// a named anchor must resolve to a painted widget; an explicitly empty anchor
/// remains the modal full-dim form.
fn draw_spotlight(
    mut egui_ctx: EguiContexts,
    mut hud: ResMut<TutorialHud>,
    anchors: Res<crate::HelpAnchors>,
    theme: Option<Res<lunco_theme::Theme>>,
    mut commands: Commands,
) {
    // A guided tour owns the scrim (see `draw_tour`); don't double-dim.
    if hud.tour.is_some() {
        return;
    }
    let Some((key, caption)) = hud.spotlight.clone() else {
        return;
    };
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let Some(screen) = tutorial_content_rect(ctx, &anchors) else {
        return;
    };
    let theme = theme
        .map(|t| t.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);
    let target = if key.is_empty() {
        None
    } else {
        tutorial_anchor_rect(&anchors, &key, screen)
    };
    if !key.is_empty() && target.is_none() {
        report_missing_anchor(&mut hud, &mut commands, &key);
        return;
    }

    let target_in_content = target.and_then(|rect| {
        let clipped = rect.intersect(screen);
        (clipped.width() > 4.0 && clipped.height() > 4.0).then_some(clipped)
    });

    if target.is_some() {
        egui::Area::new(egui::Id::new("lunco_spotlight_scrim"))
            .order(egui::Order::Background)
            .interactable(false)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                paint_scrim(
                    ui.painter(),
                    ctx,
                    screen,
                    target_in_content,
                    theme.tokens.scrim,
                    theme.tokens.accent,
                )
            });
        if let (Some(target), None) = (target, target_in_content) {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("lunco_spotlight_menu_anchor_ring"),
            ));
            paint_ring(&painter, ctx, target, theme.tokens.accent);
        }
    }

    if caption.is_empty() {
        return;
    }
    // Caption callout: just below the target (or centred when no target).
    let card_w = 300.0;
    let pos = match target {
        Some(t) => {
            let x = (t.center().x - card_w * 0.5)
                .clamp(screen.left() + 12.0, screen.right() - card_w - 12.0);
            let below = t.max.y + 14.0;
            let y = if below + 70.0 <= screen.bottom() {
                below
            } else {
                (t.min.y - 84.0).max(screen.top() + 12.0)
            };
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
                .fill(theme.tokens.overlay_backdrop)
                .corner_radius(12.0)
                .stroke(egui::Stroke::new(1.5, theme.tokens.accent))
                .inner_margin(egui::Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(&caption)
                            .color(theme.tokens.text)
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
    // Passed in rather than read via `lunco_theme::active(ctx)`: that cache is
    // only published by the Modelica canvas, so everywhere else it silently
    // returns `Theme::dark()` (see the note in `lib.rs`).
    scrim: egui::Color32,
    accent: egui::Color32,
) {
    let Some(t) = target else {
        painter.rect_filled(screen, 0.0, scrim);
        return;
    };
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
        egui::Rect::from_min_max(
            egui::pos2(screen.min.x, t.min.y),
            egui::pos2(t.min.x, t.max.y),
        ),
        0.0,
        scrim,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(t.max.x, t.min.y),
            egui::pos2(screen.max.x, t.max.y),
        ),
        0.0,
        scrim,
    );
    paint_ring(painter, ctx, t, accent);
}

/// Paint an anchor ring without requiring the target to be inside the scene
/// content rectangle. This is used for title-bar buttons, which remain visible
/// above the content while the coach card is positioned below them.
fn paint_ring(
    painter: &egui::Painter,
    ctx: &egui::Context,
    target: egui::Rect,
    accent: egui::Color32,
) {
    let phase = (ctx.input(|i| i.time).sin() as f32 * 0.5 + 0.5) * 0.55 + 0.45;
    let ring = egui::Color32::from_rgba_unmultiplied(
        accent.r(),
        accent.g(),
        accent.b(),
        (255.0 * phase) as u8,
    );
    painter.rect_stroke(
        target,
        8.0,
        egui::Stroke::new(2.5, ring),
        egui::StrokeKind::Outside,
    );
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

/// Fire a tour-navigation event on the bus. The running rhai tour advances on
/// these in its `on_event`; `data` carries the jump index for `cmd:TutorialGoto`.
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
/// controls. Controls fire `cmd:Tutorial{Next,Back,Skip,Goto}` on the bus; the
/// running rhai tour advances on them. Shared by lunica and the luncosim.
fn draw_tour(
    mut egui_ctx: EguiContexts,
    mut hud: ResMut<TutorialHud>,
    anchors: Res<crate::HelpAnchors>,
    theme: Option<Res<lunco_theme::Theme>>,
    placeholder: Option<Res<crate::viewport::ViewportPlaceholder>>,
    scene_viewport: Option<Res<lunco_core::SceneViewport>>,
    mut commands: Commands,
) {
    let Some(step) = hud.tour.clone() else { return };
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let Some(screen) = tutorial_content_rect(ctx, &anchors) else {
        return;
    };

    let theme = theme
        .map(|t| t.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);
    let accent = theme.tokens.accent;
    let accent_text = theme.colors.base;
    let muted = theme.tokens.text_subdued;
    let text = theme.colors.text;
    let card_fill = {
        let [r, g, b, _] = theme.tokens.surface_raised.to_array();
        egui::Color32::from_rgba_unmultiplied(r, g, b, 250)
    };

    let target = if step.anchor.is_empty() {
        None
    } else {
        tutorial_anchor_rect(&anchors, &step.anchor, screen)
    };
    if !step.anchor.is_empty() && target.is_none() {
        report_missing_anchor(&mut hud, &mut commands, &step.anchor);
        return;
    }

    // A UI-only lesson must retain the ordinary empty-workbench presentation.
    // There is no scene to spotlight in this state, so a full scrim would turn
    // the readable empty-viewport message into an apparent black screen.
    let empty_viewport = placeholder
        .as_ref()
        .is_some_and(|viewport| viewport.message.is_some())
        || scene_viewport
            .as_ref()
            .is_some_and(|viewport| viewport.active_camera.is_none());
    let show_scrim = !empty_viewport;
    let target_in_content = target.and_then(|rect| {
        let clipped = rect.intersect(screen);
        (clipped.width() > 4.0 && clipped.height() > 4.0).then_some(clipped)
    });

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
        let target_huge = t.width() > screen.width() * 0.55 && t.height() > screen.height() * 0.5;
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
    // An empty anchor is an intentional modal step when a scene is present.
    // With no scene, retain the normal empty-viewport presentation and draw
    // only the authored target/card.
    if target.is_some() || step.anchor.is_empty() {
        egui::Area::new(egui::Id::new("lunco_tour_scrim"))
            .order(egui::Order::Background)
            .interactable(false)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let painter = ui.painter();
                if show_scrim {
                    paint_scrim(
                        painter,
                        ctx,
                        screen,
                        target_in_content,
                        theme.tokens.scrim,
                        accent,
                    );
                } else if let Some(t) = target_in_content {
                    paint_ring(painter, ctx, t, accent);
                }
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
    }

    // Menu targets sit above `screen`, so they cannot be ringed by the content
    // scrim. Paint their ring in a foreground layer while leaving the title bar
    // itself interactive.
    if let (Some(target), None) = (target, target_in_content) {
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("lunco_tour_menu_anchor_ring"),
        ));
        paint_ring(&painter, ctx, target, accent);
    }

    // ── Card ─────────────────────────────────────────────────────────────────
    let last = step.index + 1 >= step.total;
    let mut next = false;
    let mut back = false;
    let mut skip = false;
    let mut stop = false;
    let mut goto: Option<usize> = None;

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
                        egui::CornerRadius {
                            nw: 13,
                            ne: 13,
                            sw: 0,
                            se: 0,
                        },
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
                        "INTERACTIVE TUTORIAL".to_string()
                    } else {
                        step.title.to_uppercase()
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
                                ui.painter()
                                    .rect_filled(bar, 2.0, muted.linear_multiply(0.25));
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
                                    .add_enabled_ui(step.index > 0, |ui| {
                                        crate::icon_text_button(
                                            ui,
                                            crate::UiIcon::Back,
                                            "Back",
                                            "Go to the previous step",
                                        )
                                    })
                                    .inner
                                    .on_disabled_hover_text("Already at the first step")
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
                                if ui
                                    .button(egui::RichText::new("Stop").color(muted).size(11.0))
                                    .on_hover_text("Stop this tutorial and clear its scene")
                                    .clicked()
                                {
                                    stop = true;
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let (icon, label, tooltip) = if last {
                                            (crate::UiIcon::Check, "Done", "Finish the tutorial")
                                        } else {
                                            (crate::UiIcon::Forward, "Next", "Go to the next step")
                                        };
                                        if crate::icon_text_button(ui, icon, label, tooltip)
                                            .clicked()
                                        {
                                            next = true;
                                        }
                                    },
                                );
                            });
                        });
                });
        });

    if next {
        commands.trigger(TutorialNext {});
    }
    if back {
        commands.trigger(TutorialBack {});
    }
    if skip {
        commands.trigger(TutorialSkip {});
    }
    if stop {
        commands.trigger(TutorialStopRequested);
    }
    if let Some(i) = goto {
        emit_tour(
            &mut commands,
            "cmd:TutorialGoto",
            lunco_core::TelemetryValue::I64(i as i64),
        );
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
        register_tutorial_navigation(app);
        // HUD / tour / spotlight are per-client presentation — client-local, so a
        // client-scoped tutorial scenario may drive them (see `ClientCommandPolicy`).
        use lunco_core::MarkClientLocalExt;
        app.mark_client_local::<SetHint>()
            .mark_client_local::<SetObjectives>()
            .mark_client_local::<Spotlight>()
            .mark_client_local::<ClearSpotlight>()
            .mark_client_local::<SetTourStep>()
            .mark_client_local::<ClearTour>();
        // The persistent objectives card is view-independent: it remains
        // visible when the user changes perspective. Spotlight/tour content
        // follows the authored track perspective and its anchors are view-local.
        app.add_systems(
            EguiPrimaryContextPass,
            draw_tutorial_hud.in_set(crate::ApplicationOverlayRenderSet),
        );
        app.add_systems(
            EguiPrimaryContextPass,
            (draw_spotlight, draw_tour, draw_tutorial_recovery)
                .in_set(crate::ApplicationOverlayRenderSet),
        );
    }
}
