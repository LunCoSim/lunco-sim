//! Multi-step product tour — spotlights real workbench widgets and
//! pops a callout next to each one. Auto-opens any panel a step
//! points at, via the existing `FocusPanel` command. Opens
//! `CascadedRCFilter` for the duration of the tour so the model
//! views have something real to show; closes it on hide.
//! (`CascadedRCFilter` has no MSL dependency, so it loads fast —
//! unlike `AnnotatedRocketStage`, which pulled in the Modelica
//! Standard Library and stalled the tour on first launch.)
//!
//! Anchors are screen-rects published into the
//! [`lunco_workbench::HelpAnchors`] resource each frame by whoever
//! draws the target. Missing anchor → centred callout fallback.
//!
//! Shown automatically on first launch (persisted in
//! `settings.json` under `help_overlay.seen`); reachable thereafter
//! from Help → Show Tour or F1.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_doc::DocumentId;
use lunco_settings::{AppSettingsExt, SettingsSection};
use lunco_workbench::HelpAnchors;
use serde::{Deserialize, Serialize};

/// Persisted "seen the tour" flag.
#[derive(Resource, Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct HelpOverlaySettings {
    /// User has dismissed the tour at least once.
    pub seen: bool,
}

impl SettingsSection for HelpOverlaySettings {
    const KEY: &'static str = "help_overlay";
}

/// Runtime overlay state.
#[derive(Resource, Default)]
pub struct HelpOverlayState {
    pub visible: bool,
    pub screen: usize,
    /// First-start auto-show has been attempted this session — don't
    /// re-open after the user closed it.
    auto_shown: bool,
    /// Screen whose `focus_panel` we've already triggered.
    focus_fired_for: Option<usize>,
    /// The bundled demo we opened for the duration of the tour.
    /// Closed when the user closes the tour.
    tutorial_doc: Option<DocumentId>,
    /// Lifecycle ticks: "open the demo on the next frame" and
    /// "capture the resulting active doc id the frame after". Lets
    /// the exclusive doc-manage system run independently of the
    /// egui render system.
    wants_open_doc: bool,
    wants_capture_doc: bool,
    wants_close_doc: bool,
    /// Original location of the Graphs tab before we moved it next
    /// to the demo model tab during the Graphs step. Restored when
    /// the tour closes so the user's layout returns to normal.
    saved_graphs_loc: Option<lunco_workbench::TabLocation>,
    /// Last screen for which we ran the Graphs demo move (so it
    /// fires once per visit, not every frame).
    graphs_demo_ran_for: Option<usize>,
}

struct HelpScreen {
    title: &'static str,
    /// One-sentence coachmark body — fits on 2-3 lines in a 380px card.
    body: &'static str,
    /// Anchor key in [`HelpAnchors`] to spotlight. `None` → centred.
    anchor: Option<&'static str>,
    /// Panel to auto-focus when this step opens. `PanelId` string.
    focus_panel: Option<&'static str>,
}

const SCREENS: &[HelpScreen] = &[
    HelpScreen {
        title: "Welcome to Lunica",
        body: "LunCoSim Modelica Workbench — a quick interactive tour. \
               Use ◀ ▶ or the arrow keys. Esc to skip.",
        anchor: None,
        focus_panel: None,
    },
    HelpScreen {
        title: "Twin Browser",
        body: "Your workspace lives here. A Twin is a folder of .mo files + a \
               manifest. Bundled demos and the Modelica Standard Library are \
               read-only — duplicate to edit.",
        anchor: Some("panel.lunco_twin_browser"),
        focus_panel: Some("lunco_twin_browser"),
    },
    HelpScreen {
        title: "Text · Diagram · Icon",
        body: "Four live views of the same model: 📝 Text source, 🔗 Diagram \
               (wired components), 🎨 Icon (the class's own annotation), and \
               📖 Docs.",
        anchor: Some("model_view.view_toggles"),
        focus_panel: None,
    },
    HelpScreen {
        title: "Compilation modes",
        body: "Two ways to compile and run a model:\n\
               • 🚀 Interactive — compile then step in real time; live \
                 sliders drive inputs, signals stream into Graphs.\n\
               • ⏩ Fast Run — batch, headless; integrate 0 → t_end \
                 then dump results to plot. Best for sweeps and \
                 reproducibility.",
        anchor: Some("model_view.compile_buttons"),
        focus_panel: None,
    },
    HelpScreen {
        title: "Graphs",
        body: "Multi-axis plots, per document. Pick signals, snapshot a run \
               to compare against future ones. Lives in the bottom dock by \
               default — next to Experiments, Diagnostics, Console, Journal.",
        anchor: Some("panel.modelica_plot"),
        // `modelica_plot` is an instance panel; `FocusPanel` only
        // resolves singletons. The default plot tab is opened at
        // startup, so its rect is already published.
        focus_panel: None,
    },
    HelpScreen {
        title: "Rearrange tabs",
        body: "Every tab is draggable. Drop one next to another to split \
               the area; drop it on a tab strip to add it as a sibling. \
               Great for watching signals next to the model while it runs.",
        anchor: Some("panel.modelica_plot"),
        focus_panel: None,
    },
    HelpScreen {
        title: "Experiments",
        body: "Override any parameter without editing source. Fast Run does a \
               one-shot batch; scheduled runs sweep override lists.",
        anchor: Some("panel.modelica_experiments"),
        focus_panel: Some("modelica_experiments"),
    },
    HelpScreen {
        title: "Telemetry",
        body: "Live parameters, inputs, and variable plot toggles. In \
               Interactive mode, sliders here drive inputs at simulation \
               rate.",
        anchor: Some("panel.modelica_inspector"),
        focus_panel: Some("modelica_inspector"),
    },
    HelpScreen {
        title: "Scripting — HTTP API & MCP",
        body: "Every UI action has a typed Command. POST to /api/commands, or \
               drive the workbench from an LLM agent over MCP.",
        anchor: Some("menu.help"),
        focus_panel: None,
    },
    HelpScreen {
        title: "You're set",
        body: "Reopen this tour any time from Help → Show Tour, or press F1.",
        anchor: None,
        focus_panel: None,
    },
];

pub struct HelpOverlayPlugin;

impl Plugin for HelpOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.register_settings_section::<HelpOverlaySettings>();
        app.init_resource::<HelpOverlayState>();
        app.add_systems(Startup, register_help_entry);
        app.add_systems(
            Update,
            (auto_show_on_first_start, auto_focus_target_panel, manage_tutorial_doc),
        );
        app.add_systems(
            bevy_egui::EguiPrimaryContextPass,
            render_help_overlay.after(lunco_workbench::WorkbenchRenderSet),
        );
    }
}

fn auto_show_on_first_start(
    mut state: ResMut<HelpOverlayState>,
    mut settings: ResMut<HelpOverlaySettings>,
) {
    if state.auto_shown {
        return;
    }
    state.auto_shown = true;
    if !settings.seen {
        open_tour(&mut state);
        // Mark seen as soon as the tour auto-shows — otherwise the
        // flag only flips when the user explicitly unchecks "Show on
        // next start", so closing the tour normally leaves `seen`
        // false and the tour (plus its CascadedRCFilter demo)
        // re-fires on every launch. Critical on wasm, where settings
        // persist in localStorage and tabs do not. The checkbox can
        // still set `seen = false` to re-arm the tour deliberately.
        settings.seen = true;
    }
}

/// Opening helper. Marks the tour visible and queues the
/// CascadedRCFilter open so the model views have content.
fn open_tour(state: &mut HelpOverlayState) {
    state.visible = true;
    state.screen = 0;
    state.focus_fired_for = None;
    if state.tutorial_doc.is_none() {
        state.wants_open_doc = true;
    }
}

fn auto_focus_target_panel(
    mut state: ResMut<HelpOverlayState>,
    mut commands: Commands,
) {
    if !state.visible {
        state.focus_fired_for = None;
        return;
    }
    if state.focus_fired_for == Some(state.screen) {
        return;
    }
    state.focus_fired_for = Some(state.screen);
    let screen = match SCREENS.get(state.screen) {
        Some(s) => s,
        None => return,
    };
    if let Some(panel_id) = screen.focus_panel {
        commands.trigger(lunco_workbench::FocusPanel {
            id: panel_id.to_string(),
        });
    }
}

/// Exclusive system: opens CascadedRCFilter when the tour starts,
/// captures the resulting active doc id, and closes the doc when the
/// tour ends. Runs as `&mut World` because `open_class` and the
/// close intent both need it.
fn manage_tutorial_doc(world: &mut World) {
    // Step 1 — fire the open. Next-frame capture flag is set so we
    // pick up the new active doc once the open-observer has run.
    let wants_open = world.resource::<HelpOverlayState>().wants_open_doc;
    if wants_open {
        crate::ui::panels::package_browser::open_class(
            world,
            crate::class_ref::ClassRef::bundled(["CascadedRCFilter"]),
            false,
        );
        let mut state = world.resource_mut::<HelpOverlayState>();
        state.wants_open_doc = false;
        state.wants_capture_doc = true;
        return;
    }

    // Step 2 — capture. Reads whatever active_document the open
    // observer landed on. The open may resolve a frame or more
    // late (the bundled source still has to parse), so keep the
    // capture flag armed until a doc actually shows up — otherwise
    // a fast skip leaves `tutorial_doc` None and the demo leaks.
    let wants_capture = world.resource::<HelpOverlayState>().wants_capture_doc;
    if wants_capture {
        let active = world
            .resource::<lunco_workbench::WorkspaceResource>()
            .active_document;
        let mut state = world.resource_mut::<HelpOverlayState>();
        if active.is_some() {
            state.tutorial_doc = active;
            state.wants_capture_doc = false;
        }
    }

    // Step 3 — close. Triggers `EditorIntent::Close` while the tour
    // doc is active; the doc's own observer handles tab teardown.
    let wants_close = world.resource::<HelpOverlayState>().wants_close_doc;
    if wants_close {
        // Restore the Graphs tab to its original spot first — has to
        // happen before we close the tour doc because some tab
        // locations are relative to nodes that the close may collapse.
        let saved = world.resource::<HelpOverlayState>().saved_graphs_loc;
        if let Some(loc) = saved {
            if let Some(mut layout) =
                world.get_resource_mut::<lunco_workbench::WorkbenchLayout>()
            {
                if let Some(plot_tab) =
                    layout.find_any_instance(crate::ui::panels::graphs::MODELICA_PLOT_KIND)
                {
                    layout.restore_tab_to(plot_tab, loc);
                }
            }
            let mut state = world.resource_mut::<HelpOverlayState>();
            state.saved_graphs_loc = None;
            state.graphs_demo_ran_for = None;
        }

        // Prefer the captured id; if capture never landed (fast
        // skip before the open resolved), fall back to whatever the
        // tour left active so the demo is still torn down.
        let (captured, pending) = {
            let s = world.resource::<HelpOverlayState>();
            (s.tutorial_doc, s.wants_capture_doc)
        };
        let doc = captured.or_else(|| {
            if pending {
                world
                    .resource::<lunco_workbench::WorkspaceResource>()
                    .active_document
            } else {
                None
            }
        });
        if let Some(doc) = doc {
            // Make the tour doc active so `EditorIntent::Close` (which
            // closes the active document) targets the right one.
            world
                .resource_mut::<lunco_workbench::WorkspaceResource>()
                .active_document = Some(doc);
            world.trigger(lunco_doc_bevy::EditorIntent::Close);
        }
        let mut state = world.resource_mut::<HelpOverlayState>();
        state.tutorial_doc = None;
        state.wants_close_doc = false;
        // Disarm capture too — a late open resolving after teardown
        // must not re-adopt a doc the tour no longer owns.
        state.wants_capture_doc = false;
    }

    // Step 4 — Graphs-step demo: programmatically move the plot tab
    // next to the demo model-view tab so the user sees that
    // tabs can be drag-rearranged. Fires once when entering the
    // Graphs step; restoration happens in Step 3 above on tour close.
    let (visible, screen, already_ran) = {
        let s = world.resource::<HelpOverlayState>();
        (s.visible, s.screen, s.graphs_demo_ran_for == Some(s.screen))
    };
    if visible && screen == GRAPHS_STEP_INDEX && !already_ran {
        let model_view_kind = crate::ui::panels::model_view::MODEL_VIEW_KIND;
        let plot_kind = crate::ui::panels::graphs::MODELICA_PLOT_KIND;
        let mut saved: Option<lunco_workbench::TabLocation> = None;
        if let Some(mut layout) =
            world.get_resource_mut::<lunco_workbench::WorkbenchLayout>()
        {
            if let (Some(model_view_tab), Some(plot_tab)) = (
                layout.find_any_instance(model_view_kind),
                layout.find_any_instance(plot_kind),
            ) {
                saved = layout.move_tab_next_to(plot_tab, model_view_tab);
            }
        }
        let mut state = world.resource_mut::<HelpOverlayState>();
        state.graphs_demo_ran_for = Some(screen);
        if state.saved_graphs_loc.is_none() {
            state.saved_graphs_loc = saved;
        }
    }
}

/// Index of the "Rearrange tabs" step in [`SCREENS`] — the one where
/// we programmatically move the Graphs tab next to the demo model so
/// the user sees that the dock is interactive. Update if screens
/// reorder. (The plain "Graphs" step at index 4 shows the panel in
/// its default bottom-dock location; the move only happens on entry
/// to this step.)
const GRAPHS_STEP_INDEX: usize = 5;

fn register_help_entry(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>()
    else {
        return;
    };
    layout.register_help_menu(|ui, world| {
        if ui.button("🎓 Show Tour").on_hover_text("Replay the guided interactive tour of the workbench").clicked() {
            let mut state = world.resource_mut::<HelpOverlayState>();
            open_tour(&mut state);
            ui.close();
        }
    });
}

/// Side of the spotlight target that the callout sits on. Picks up
/// where to draw the speech-bubble tail.
#[derive(Clone, Copy)]
enum CalloutSide {
    Right,
    Below,
    Above,
    Left,
    /// Card sits ON the highlighted panel — used when the panel is
    /// tall but too narrow to place a card alongside (typical for
    /// side-browser style panels). Tail points down to the panel's
    /// center vertical so the connection still reads clearly.
    Over,
    /// Centred — no target. No tail drawn.
    Centred,
}

fn render_help_overlay(
    mut egui_ctx: bevy_egui::EguiContexts,
    mut state: ResMut<HelpOverlayState>,
    mut settings: ResMut<HelpOverlaySettings>,
    anchors: Res<HelpAnchors>,
    theme: Option<Res<lunco_theme::Theme>>,
) {
    let Ok(ctx) = egui_ctx.ctx_mut() else {
        return;
    };

    if !state.visible {
        if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            open_tour(&mut state);
        }
        return;
    }

    let total = SCREENS.len();
    if state.screen >= total {
        state.screen = total - 1;
    }
    let screen = &SCREENS[state.screen];

    let theme = theme
        .map(|t| t.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);
    let accent = theme.tokens.accent;
    let accent_text = theme.colors.base;
    let muted = theme.tokens.text_subdued;
    let text = theme.colors.text;
    let surface_raised = theme.tokens.surface_raised;

    let viewport = ctx.content_rect();
    let mut close = false;
    let mut goto: Option<usize> = None;

    let target_rect = screen
        .anchor
        .and_then(|k| anchors.get(k))
        .map(|r| r.expand(6.0).intersect(viewport))
        .filter(|r| r.width() > 4.0 && r.height() > 4.0);

    // Pre-compute card position + which side it sits on so the
    // scrim layer can paint the speech-bubble tail before the card
    // covers it.
    let card_w = 380.0;
    let card_h_est = 260.0;
    let margin = 18.0;
    let (side, card_pos) = if let Some(t) = target_rect {
        // "Over" — card sits *inside* the highlighted panel, near
        // the top. Used for narrow side panels (Twin Browser) and
        // for large central panels (Model View) where placing the
        // card to the side leaves it floating in dead space far
        // from the actual content.
        let over_pos = egui::pos2(
            (t.center().x - card_w * 0.5).clamp(
                viewport.min.x + margin,
                viewport.max.x - card_w - margin,
            ),
            (t.min.y + 16.0).clamp(
                viewport.min.y + margin,
                viewport.max.y - card_h_est - margin,
            ),
        );
        // Only the *huge* central-panel case prefers "Over" (the
        // card sits inside the panel). Narrow side panels are
        // *better* served by a Right-placed card with a tail —
        // putting the card "Over" a 200px-wide Twin Browser pushes
        // it into the center area anyway and reads as misplaced.
        let target_huge = t.width() > viewport.width() * 0.55
            && t.height() > viewport.height() * 0.5;
        let prefer_over = target_huge;

        // Short targets (toolbar / menu buttons) need extra
        // clearance below — placing the card at `target.max.y +
        // 18px` lands it directly on the dock's tab strip, which
        // reads as broken UI. Drop it past the strip into the
        // panel content area; the tail stretches to compensate.
        let target_short = t.height() < 50.0;
        let below_y = if target_short {
            (t.max.y + 80.0).clamp(
                viewport.min.y + margin,
                viewport.max.y - card_h_est - margin,
            )
        } else {
            t.max.y + margin
        };

        let side_candidates = [
            (
                CalloutSide::Right,
                egui::pos2(
                    t.max.x + margin,
                    (t.center().y - card_h_est * 0.5).clamp(
                        viewport.min.y + margin,
                        viewport.max.y - card_h_est - margin,
                    ),
                ),
            ),
            (
                CalloutSide::Below,
                egui::pos2(
                    (t.center().x - card_w * 0.5).clamp(
                        viewport.min.x + margin,
                        viewport.max.x - card_w - margin,
                    ),
                    below_y,
                ),
            ),
            (
                CalloutSide::Above,
                egui::pos2(
                    (t.center().x - card_w * 0.5).clamp(
                        viewport.min.x + margin,
                        viewport.max.x - card_w - margin,
                    ),
                    t.min.y - card_h_est - margin,
                ),
            ),
            (
                CalloutSide::Left,
                egui::pos2(
                    t.min.x - card_w - margin,
                    (t.center().y - card_h_est * 0.5).clamp(
                        viewport.min.y + margin,
                        viewport.max.y - card_h_est - margin,
                    ),
                ),
            ),
        ];
        let fits = |p: &egui::Pos2| {
            p.x >= viewport.min.x + margin
                && p.x + card_w <= viewport.max.x - margin
                && p.y >= viewport.min.y + margin
                && p.y + card_h_est <= viewport.max.y - margin
        };
        if prefer_over {
            (CalloutSide::Over, over_pos)
        } else {
            side_candidates
                .into_iter()
                .find(|(_, p)| fits(p))
                .unwrap_or((CalloutSide::Over, over_pos))
        }
    } else {
        (
            CalloutSide::Centred,
            egui::pos2(
                viewport.center().x - card_w * 0.5,
                viewport.center().y - card_h_est * 0.5,
            ),
        )
    };

    // Card fill — translucent surface_raised. Used by the scrim's
    // tail painter and the callout frame so the seam is invisible.
    let card_fill = {
        let [r, g, b, _] = surface_raised.to_array();
        egui::Color32::from_rgba_unmultiplied(r, g, b, 250)
    };

    // ── Scrim layer ────────────────────────────────────────────────
    egui::Area::new(egui::Id::new("modelica_help_scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(viewport.min)
        .interactable(true)
        .show(ctx, |ui| {
            let (full, resp) =
                ui.allocate_exact_size(viewport.size(), egui::Sense::click());
            let painter = ui.painter();
            let scrim = egui::Color32::from_black_alpha(180);
            if let Some(t) = target_rect {
                painter.rect_filled(
                    egui::Rect::from_min_max(full.min, egui::pos2(full.max.x, t.min.y)),
                    0.0,
                    scrim,
                );
                painter.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(full.min.x, t.max.y), full.max),
                    0.0,
                    scrim,
                );
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(full.min.x, t.min.y),
                        egui::pos2(t.min.x, t.max.y),
                    ),
                    0.0,
                    scrim,
                );
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(t.max.x, t.min.y),
                        egui::pos2(full.max.x, t.max.y),
                    ),
                    0.0,
                    scrim,
                );
                // Pulsing accent ring around the cutout.
                let phase =
                    (ctx.input(|i| i.time).sin() as f32 * 0.5 + 0.5) * 0.55 + 0.45;
                let ring_color = egui::Color32::from_rgba_unmultiplied(
                    accent.r(),
                    accent.g(),
                    accent.b(),
                    (255.0 * phase) as u8,
                );
                painter.rect_stroke(
                    t,
                    8.0,
                    egui::Stroke::new(2.5, ring_color),
                    egui::StrokeKind::Outside,
                );

                // Speech-bubble tail — drawn here, BEHIND the card
                // (the card is in Tooltip-order on top of this).
                // Card-fill triangle with the same accent outline as
                // the card border so the seam vanishes.
                let card_rect = egui::Rect::from_min_size(
                    card_pos,
                    egui::vec2(card_w, card_h_est),
                );
                if let Some((apex, b1, b2)) =
                    tail_points(side, t, card_rect, accent)
                {
                    use egui::epaint::PathShape;
                    painter.add(egui::Shape::Path(PathShape {
                        points: vec![apex, b1, b2],
                        closed: true,
                        fill: card_fill,
                        stroke: egui::Stroke::new(
                            1.0,
                            accent.linear_multiply(0.55),
                        )
                        .into(),
                    }));
                }

                ctx.request_repaint();
            } else {
                painter.rect_filled(full, 0.0, scrim);
            }
            if resp.clicked() {
                close = true;
            }
        });

    // ── Callout card ──────────────────────────────────────────────
    egui::Area::new(egui::Id::new("modelica_help_callout"))
        .order(egui::Order::Tooltip)
        .fixed_pos(card_pos)
        .interactable(true)
        .show(ctx, |ui| {
            ui.set_width(card_w);

            // Drop-shadow: paint a faint dark rect under the card.
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    card_pos + egui::vec2(0.0, 6.0),
                    egui::vec2(card_w, card_h_est),
                ),
                14.0,
                egui::Color32::from_black_alpha(110),
            );

            egui::Frame::new()
                .fill(card_fill)
                .corner_radius(14.0)
                .inner_margin(egui::Margin::ZERO)
                .stroke(egui::Stroke::new(1.5, accent))
                .show(ui, |ui| {
                    // ── Banner — full-width accent stripe ─────────
                    // Visually unmistakable: this is a tutorial card,
                    // not a settings dialog.
                    let banner_h = 32.0;
                    let (banner_rect, _) = ui.allocate_exact_size(
                        egui::vec2(card_w, banner_h),
                        egui::Sense::hover(),
                    );
                    let p = ui.painter();
                    p.rect_filled(
                        egui::Rect::from_min_max(
                            banner_rect.min,
                            egui::pos2(banner_rect.max.x, banner_rect.max.y),
                        ),
                        egui::CornerRadius {
                            nw: 13,
                            ne: 13,
                            sw: 0,
                            se: 0,
                        },
                        accent,
                    );
                    // Diagonal pinstripes — playful texture so the
                    // banner doesn't read as a flat header.
                    let stripe = accent_text.linear_multiply(0.12);
                    let step = 10.0;
                    let mut x = banner_rect.min.x - banner_h;
                    while x < banner_rect.max.x {
                        p.line_segment(
                            [
                                egui::pos2(x, banner_rect.max.y),
                                egui::pos2(x + banner_h, banner_rect.min.y),
                            ],
                            egui::Stroke::new(1.5, stripe),
                        );
                        x += step;
                    }
                    // First step keeps the generic tour-introduction
                    // label so users immediately recognise what the
                    // overlay is. Every subsequent step swaps in the
                    // step's title so the banner reads as a section
                    // header (e.g. "Telemetry").
                    let banner_label = if state.screen == 0 {
                        "🎓  INTERACTIVE TUTORIAL".to_string()
                    } else {
                        format!("🎓  {}", screen.title.to_uppercase())
                    };
                    p.text(
                        banner_rect.min + egui::vec2(14.0, banner_h * 0.5),
                        egui::Align2::LEFT_CENTER,
                        banner_label,
                        egui::FontId::proportional(12.5),
                        accent_text,
                    );
                    p.text(
                        banner_rect.max - egui::vec2(14.0, banner_h * 0.5),
                        egui::Align2::RIGHT_CENTER,
                        format!("Step {} / {}", state.screen + 1, total),
                        egui::FontId::proportional(11.5),
                        accent_text,
                    );

                    // Body padding.
                    ui.add_space(2.0);
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(18, 14))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(screen.body)
                                    .size(12.5)
                                    .color(text),
                            );
                            ui.add_space(10.0);

                            // Progress bar.
                            let (bar_rect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 4.0),
                                egui::Sense::hover(),
                            );
                            ui.painter().rect_filled(
                                bar_rect,
                                2.0,
                                muted.linear_multiply(0.25),
                            );
                            let frac =
                                (state.screen as f32 + 1.0) / total as f32;
                            let fill_rect = egui::Rect::from_min_max(
                                bar_rect.min,
                                egui::pos2(
                                    bar_rect.min.x + bar_rect.width() * frac,
                                    bar_rect.max.y,
                                ),
                            );
                            ui.painter().rect_filled(fill_rect, 2.0, accent);
                            ui.add_space(8.0);

                            // Dot strip.
                            ui.horizontal_wrapped(|ui| {
                                for i in 0..total {
                                    let active = i == state.screen;
                                    let done = i < state.screen;
                                    let color = if active {
                                        accent
                                    } else if done {
                                        accent.linear_multiply(0.5)
                                    } else {
                                        muted.linear_multiply(0.4)
                                    };
                                    let (dot_rect, resp) = ui.allocate_exact_size(
                                        egui::vec2(14.0, 14.0),
                                        egui::Sense::click(),
                                    );
                                    ui.painter().circle_filled(
                                        dot_rect.center(),
                                        if active { 5.0 } else { 3.5 },
                                        color,
                                    );
                                    if resp.clicked() {
                                        goto = Some(i);
                                    }
                                    resp.on_hover_text(SCREENS[i].title);
                                }
                            });
                            ui.add_space(10.0);

                            // Buttons row.
                            ui.horizontal(|ui| {
                                let on_first = state.screen == 0;
                                let on_last = state.screen + 1 >= total;
                                if ui
                                    .add_enabled(
                                        !on_first,
                                        egui::Button::new("◀  Back")
                                            .min_size(egui::vec2(80.0, 28.0)),
                                    )
                                    .clicked()
                                {
                                    goto = Some(state.screen - 1);
                                }
                                if ui
                                    .button(
                                        egui::RichText::new("Skip")
                                            .color(muted)
                                            .size(11.0),
                                    )
                                    .clicked()
                                {
                                    close = true;
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(
                                        egui::Align::Center,
                                    ),
                                    |ui| {
                                        let label = if on_last {
                                            "Done ✓"
                                        } else {
                                            "Next  ▶"
                                        };
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(label)
                                                        .strong()
                                                        .color(accent_text),
                                                )
                                                .fill(accent)
                                                .min_size(egui::vec2(
                                                    90.0, 28.0,
                                                )),
                                            )
                                            .clicked()
                                        {
                                            if on_last {
                                                close = true;
                                            } else {
                                                goto = Some(state.screen + 1);
                                            }
                                        }
                                    },
                                );
                            });
                            ui.add_space(4.0);

                            let mut show_next_start = !settings.seen;
                            if ui
                                .checkbox(
                                    &mut show_next_start,
                                    "Show on next start",
                                )
                                .on_hover_text(
                                    "Re-open this tour automatically next \
                                     time you launch LunCoSim.",
                                )
                                .changed()
                            {
                                settings.seen = !show_next_start;
                            }
                        });
                });
        });

    ctx.input(|i| {
        if i.key_pressed(egui::Key::ArrowRight) && state.screen + 1 < total {
            goto = Some(state.screen + 1);
        }
        if i.key_pressed(egui::Key::ArrowLeft) && state.screen > 0 {
            goto = Some(state.screen - 1);
        }
        if i.key_pressed(egui::Key::Escape) {
            close = true;
        }
    });

    if let Some(s) = goto {
        if s != state.screen {
            state.screen = s;
            state.focus_fired_for = None;
        }
    }
    if close {
        state.visible = false;
        state.focus_fired_for = None;
        // Fire the cleanup pass unconditionally. The handler closes
        // the tutorial doc (if any) and restores the Graphs tab to
        // its pre-tour location (if we moved it). Either side may
        // be a no-op; the flag just guarantees we run *some* tear-
        // down on every tour close.
        state.wants_close_doc = true;
    }
}

/// Three points of the speech-bubble tail triangle: apex (tip near
/// the target) and two base points anchored to the card edge.
fn tail_points(
    side: CalloutSide,
    target: egui::Rect,
    card: egui::Rect,
    _accent: egui::Color32,
) -> Option<(egui::Pos2, egui::Pos2, egui::Pos2)> {
    let base_half = 10.0;
    Some(match side {
        CalloutSide::Right => {
            let edge_x = card.min.x;
            let cy = target.center().y.clamp(
                card.min.y + base_half + 4.0,
                card.max.y - base_half - 4.0,
            );
            (
                egui::pos2(target.max.x, cy),
                egui::pos2(edge_x + 0.5, cy - base_half),
                egui::pos2(edge_x + 0.5, cy + base_half),
            )
        }
        CalloutSide::Left => {
            let edge_x = card.max.x;
            let cy = target.center().y.clamp(
                card.min.y + base_half + 4.0,
                card.max.y - base_half - 4.0,
            );
            (
                egui::pos2(target.min.x, cy),
                egui::pos2(edge_x - 0.5, cy - base_half),
                egui::pos2(edge_x - 0.5, cy + base_half),
            )
        }
        CalloutSide::Below => {
            let edge_y = card.min.y;
            let cx = target.center().x.clamp(
                card.min.x + base_half + 4.0,
                card.max.x - base_half - 4.0,
            );
            (
                egui::pos2(cx, target.max.y),
                egui::pos2(cx - base_half, edge_y + 0.5),
                egui::pos2(cx + base_half, edge_y + 0.5),
            )
        }
        CalloutSide::Above => {
            let edge_y = card.max.y;
            let cx = target.center().x.clamp(
                card.min.x + base_half + 4.0,
                card.max.x - base_half - 4.0,
            );
            (
                egui::pos2(cx, target.min.y),
                egui::pos2(cx - base_half, edge_y - 0.5),
                egui::pos2(cx + base_half, edge_y - 0.5),
            )
        }
        CalloutSide::Over => return None,
        CalloutSide::Centred => return None,
    })
}
