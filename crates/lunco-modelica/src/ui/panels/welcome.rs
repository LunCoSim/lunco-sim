//! Welcome tab — the app's landing page.
//!
//! Layout, top → bottom:
//!
//! 1. **Hero** — product name + tagline.
//! 2. **Get Started** — New Model · Open Folder buttons.
//! 3. **Learning paths** — three hand-authored tutorial paths
//!    (Circuits 101, Control basics, Moving parts), each a sequence
//!    of 5 MSL classes with a one-line goal. Progress dots
//!    (⚪ not opened · ✅ opened) drive a subtle game-feel;
//!    `opened` state is read from the persisted `ExampleProgress`
//!    ledger (bumped by the `OpenClass` observer in
//!    `welcome_progress.rs`). Paths expand inline on click so only
//!    one is open at a time — avoids the "wall of cards" feel of
//!    the earlier draft.
//! 4. **Browse all examples** — collapsed by default, behind a
//!    `CollapsingHeader`. Search box + domain chips + 2-col card
//!    grid over the full ~700 MSL examples. For power-users who
//!    know what they want; no progress tracking here.
//! 5. **Shortcuts footer**.
//!
//! Every example — bundled or MSL — opens as a **read-only tab** via
//! `OpenClass`. The canvas read-only guard surfaces an explanation
//! on first edit attempt (see `canvas_diagram::apply_ops`).
//!
//! The panel is non-closable so the dock layout always has a center
//! anchor — even with no tabs open, the user has somewhere to land.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};

use crate::models::BUNDLED_MODELS;
use crate::ui::state::ModelLibrary;
use crate::ui::welcome_progress::ExampleProgress;
use crate::visual_diagram::{msl_component_library, MSLComponentDef};

/// Panel id.
pub const WELCOME_PANEL_ID: PanelId = PanelId("modelica_welcome");

/// One step in a learning path — a concrete MSL class and the
/// human-authored goal that tells the learner *what to look for*
/// when they hit Run. Keeping the goal short (< 90 chars) keeps
/// cards visually uniform and encourages punchy copy.
struct PathStep {
    qualified: &'static str,
    /// Short display name (falls back to the trailing segment of
    /// `qualified` when empty).
    label: &'static str,
    goal: &'static str,
}

/// A hand-curated tutorial arc across ~5 MSL examples.
struct LearningPath {
    icon: &'static str,
    title: &'static str,
    subtitle: &'static str,
    steps: &'static [PathStep],
}

/// The three beginner paths. Intentionally small — more than three
/// is paradox-of-choice; four examples per path is the classic
/// tutorial rhythm (setup → variation → extension → integration).
/// Adding a path? Keep `steps.len()` in the 4..=6 range and lead
/// with the simplest, most visual example.
const PATHS: &[LearningPath] = &[
    LearningPath {
        icon: "⚡",
        title: "Circuits 101",
        subtitle: "From a passive filter to an op-amp stage.",
        steps: &[
            PathStep {
                qualified: "Modelica.Electrical.Analog.Examples.ChuaCircuit",
                label: "ChuaCircuit",
                goal: "Run it. Watch the chaotic attractor form on the XY plot.",
            },
            PathStep {
                qualified: "Modelica.Electrical.Analog.Examples.CauerLowPassAnalog",
                label: "CauerLowPass",
                goal: "5th-order Cauer filter. Compare input to filtered output.",
            },
            PathStep {
                qualified: "Modelica.Electrical.Analog.Examples.Rectifier",
                label: "Rectifier",
                goal: "Turn AC into pulsed DC. Overlay grid + load voltage.",
            },
            PathStep {
                qualified: "Modelica.Electrical.Analog.Examples.ShowSaturatingInductor",
                label: "SaturatingInductor",
                goal: "Inductor hits its B-H knee. See current go non-linear.",
            },
            PathStep {
                qualified: "Modelica.Electrical.Analog.Examples.AmplifierWithOpAmpDetailed",
                label: "OpAmpDetailed",
                goal: "Amplify a small sine. Tune the feedback ratio, re-run.",
            },
        ],
    },
    LearningPath {
        icon: "⚙",
        title: "Control basics",
        subtitle: "Feedback loops, filters, boolean plumbing.",
        steps: &[
            PathStep {
                qualified: "Modelica.Blocks.Examples.PID_Controller",
                label: "PID_Controller",
                goal: "Step change in setpoint. Watch P-I-D tame the response.",
            },
            PathStep {
                qualified: "Modelica.Blocks.Examples.FilterWithRiseTime",
                label: "FilterWithRiseTime",
                goal: "Low-pass with a rise-time spec. Time the 10→90% climb.",
            },
            PathStep {
                qualified: "Modelica.Blocks.Examples.InverseModel",
                label: "InverseModel",
                goal: "Invert a transfer function live. The classic trick.",
            },
            PathStep {
                qualified: "Modelica.Blocks.Examples.RealNetwork1",
                label: "RealNetwork1",
                goal: "Arithmetic blocks wired into a tiny network.",
            },
            PathStep {
                qualified: "Modelica.Blocks.Examples.LogicalNetwork1",
                label: "LogicalNetwork1",
                goal: "Boolean gates reacting to pulses. Read the truth table.",
            },
        ],
    },
    LearningPath {
        icon: "🔧",
        title: "Moving parts",
        subtitle: "Mechanics from a spring-damper to a 3D pendulum.",
        steps: &[
            PathStep {
                qualified: "Modelica.Mechanics.Rotational.Examples.First",
                label: "Rotational.First",
                goal: "Torque through inertia + spring-damper. The hello world.",
            },
            PathStep {
                qualified: "Modelica.Mechanics.Rotational.Examples.ElasticBearing",
                label: "ElasticBearing",
                goal: "Shaft flex under load. Watch the phase lag develop.",
            },
            PathStep {
                qualified: "Modelica.Mechanics.Rotational.Examples.CoupledClutches",
                label: "CoupledClutches",
                goal: "Clutches engage in sequence. Follow torque hand-off.",
            },
            PathStep {
                qualified: "Modelica.Mechanics.Translational.Examples.Damper",
                label: "Translational.Damper",
                goal: "Mass-spring-damper in 1D. Change c, see the overshoot shift.",
            },
            PathStep {
                qualified: "Modelica.Mechanics.MultiBody.Examples.Elementary.DoublePendulum",
                label: "DoublePendulum",
                goal: "Chaotic 3D two-link pendulum. Tweak initial angle.",
            },
        ],
    },
];

/// Per-panel state: which path (if any) is expanded, plus the
/// Browse-all search/domain state. Stored in egui's data bag so the
/// render closure can read/write without a Bevy resource — Welcome
/// is a singleton panel so a single key is safe.
#[derive(Clone, Default)]
struct WelcomeState {
    /// Index into `PATHS` of the expanded path. `None` = all
    /// collapsed.
    expanded: Option<usize>,
    /// Live search string for Browse-all.
    browse_query: String,
    /// Selected domain chip in Browse-all. Empty = "All".
    browse_domain: String,
}

const STATE_ID: &str = "modelica_welcome_state_v2";

fn domain_icon(domain: &str) -> &'static str {
    match domain {
        "Electrical" => "⚡",
        "Mechanics" => "🔧",
        "Fluid" => "💧",
        "Thermal" => "🔥",
        // Use widely-rendered glyphs instead of the newer emoji
        // that turn into tofu in the bundled DejaVu Sans fallback:
        // 🧲/🧮/🛠/⏱ tend to miss, ⊗/Σ/⚒/⧖ are in basic math
        // plane and present in every sans font we've loaded.
        "Magnetic" => "⊗",
        "Blocks" => "⚙",
        "ComplexBlocks" => "⚙",
        "Math" => "Σ",
        "StateGraph" => "⇄",
        "Clocked" => "⧖",
        "Media" => "◆",
        "Utilities" => "⚒",
        _ => "📦",
    }
}

fn is_top_level_example(c: &MSLComponentDef) -> bool {
    if !c.is_example {
        return false;
    }
    let mut parts = c.msl_path.rsplit('.');
    let _leaf = parts.next();
    matches!(parts.next(), Some("Examples"))
}

fn card_subtitle(c: &MSLComponentDef) -> String {
    if let Some(s) = c.short_description.as_ref() {
        if !s.is_empty() {
            return s.clone();
        }
    }
    if let Some(info) = c.documentation_info.as_ref() {
        if let Some(end) = info.find(". ") {
            return format!("{}.", &info[..end]);
        }
        return info.clone();
    }
    String::new()
}

pub struct WelcomePanel;

impl Panel for WelcomePanel {
    fn id(&self) -> PanelId {
        WELCOME_PANEL_ID
    }

    fn title(&self) -> String {
        "🏠 Welcome".into()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn closable(&self) -> bool {
        false
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let mut create_new = false;
        let mut open_folder = false;
        let mut open_msl: Option<String> = None;
        let mut open_bundled: Option<&'static str> = None;

        // Theme tokens.
        let theme = world
            .get_resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        let card_fill = theme.colors.surface0;
        let card_stroke = theme.colors.surface2;
        let chip_fill_active = theme.tokens.accent;
        let chip_fill_idle = theme.colors.surface1;
        let chip_text_active = theme.colors.base;
        let chip_text_idle = theme.colors.text;
        let title_tint = theme.tokens.accent;
        let muted = theme.tokens.text_subdued;
        let success = theme.tokens.success;

        // Progress ledger snapshot — cloned so the render closure
        // doesn't hold a live borrow of the Bevy resource across
        // the egui closures that fire command triggers.
        let progress: ExampleProgress = world
            .get_resource::<ExampleProgress>()
            .cloned()
            .unwrap_or_default();

        // Pull + mutate per-panel UI state from egui's data bag.
        let state_id = egui::Id::new(STATE_ID);
        let mut wstate: WelcomeState = ui
            .ctx()
            .data_mut(|d| d.get_temp::<WelcomeState>(state_id).unwrap_or_default());

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(32.0);

            // ── Hero ───────────────────────────────────────
            ui.vertical_centered(|ui| {
                ui.heading(
                    egui::RichText::new("LunCoSim Modelica Workbench")
                        .size(24.0),
                );
                ui.label(
                    egui::RichText::new(
                        "Build physics models, simulate them, see the numbers.",
                    )
                    .size(13.0)
                    .color(muted),
                );
            });

            ui.add_space(24.0);

            // ── Get Started ────────────────────────────────
            ui.vertical_centered(|ui| {
                ui.set_max_width(560.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_sized(
                            [272.0, 44.0],
                            egui::Button::new(
                                egui::RichText::new("➕  New Model")
                                    .size(14.0)
                                    .strong(),
                            ),
                        )
                        .on_hover_text("Create a new untitled model (Ctrl+N)")
                        .clicked()
                    {
                        create_new = true;
                    }
                    if ui
                        .add_sized(
                            [272.0, 44.0],
                            egui::Button::new(
                                egui::RichText::new("📁  Open Folder")
                                    .size(14.0)
                                    .strong(),
                            ),
                        )
                        .on_hover_text("Pick a folder of .mo files to browse")
                        .clicked()
                    {
                        open_folder = true;
                    }
                });
            });

            ui.add_space(32.0);

            // ── LunCoSim demos (bundled) ───────────────────
            // Our own authored starters. Separate from the MSL
            // `.Examples.*` set because these files live
            // in-repo (`assets/models/*.mo`), are small, and
            // showcase LunCoSim-specific annotation fixtures
            // (rocket stage, battery cell, etc.). They open via
            // the `open_model` bundled-library path rather than
            // `OpenClass`.
            ui.vertical_centered(|ui| {
                let w = ui.available_width().min(760.0);
                ui.set_max_width(w);
                // Collapsed by default — Learning paths are the
                // primary onboarding; demos are the "explore our
                // own authored models" escape hatch. Keeps the
                // fold above scroll dominated by the guided
                // content without hiding the demos entirely.
                egui::CollapsingHeader::new(
                    egui::RichText::new(format!(
                        "LunCoSim demos ({} bundled)",
                        BUNDLED_MODELS.len()
                    ))
                    .size(14.0)
                    .color(title_tint),
                )
                .id_salt("welcome_bundled_demos")
                .default_open(false)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(
                            "Our in-house starters — small, annotated, \
                             read-only. Duplicate to edit.",
                        )
                        .size(11.0)
                        .color(muted),
                    );
                    ui.add_space(8.0);

                    // 2-col grid of bundled cards.
                    let col_w = ((ui.available_width() - 8.0) / 2.0).max(220.0);
                let row_h = 52.0;
                let mut iter = BUNDLED_MODELS.iter();
                loop {
                    let left = iter.next();
                    let right = iter.next();
                    if left.is_none() {
                        break;
                    }
                    ui.horizontal(|ui| {
                        for entry in [left, right].into_iter().flatten() {
                            let display = entry
                                .filename
                                .strip_suffix(".mo")
                                .unwrap_or(entry.filename);
                            let resp = ui
                                .add_sized(
                                    [col_w, row_h],
                                    egui::Button::new("")
                                        .fill(card_fill)
                                        .stroke(egui::Stroke::new(
                                            1.0,
                                            card_stroke,
                                        )),
                                )
                                .on_hover_text(format!(
                                    "Open {} as a read-only tab",
                                    display
                                ));
                            let rect = resp.rect;
                            let painter = ui.painter_at(rect);
                            painter.text(
                                rect.min + egui::vec2(14.0, 8.0),
                                egui::Align2::LEFT_TOP,
                                format!("📄  {}", display),
                                egui::FontId::proportional(13.5),
                                title_tint,
                            );
                            // Trim tagline so both cards stay
                            // one-line and visually aligned.
                            let tagline = entry.tagline;
                            let tagline = if tagline.chars().count() > 64 {
                                let mut s: String =
                                    tagline.chars().take(64).collect();
                                s.push('…');
                                s
                            } else {
                                tagline.to_string()
                            };
                            painter.text(
                                rect.min + egui::vec2(14.0, 28.0),
                                egui::Align2::LEFT_TOP,
                                tagline,
                                egui::FontId::proportional(10.5),
                                muted,
                            );
                            if resp.clicked() {
                                open_bundled = Some(entry.filename);
                            }
                        }
                    });
                    ui.add_space(6.0);
                }
                });
            });

            ui.add_space(24.0);

            // ── Learning Paths ─────────────────────────────
            ui.vertical_centered(|ui| {
                // Adaptive width — honour 760px as the comfort-read
                // ceiling but shrink to fit narrow panels so the
                // left edge doesn't clip under the dock rail.
                let w = ui.available_width().min(760.0);
                ui.set_max_width(w);

                // Overall header — "X of N paths started" turns the
                // section into a tiny dashboard.
                let total_steps: usize =
                    PATHS.iter().map(|p| p.steps.len()).sum();
                let done_steps: usize = PATHS
                    .iter()
                    .flat_map(|p| p.steps.iter())
                    .filter(|s| progress.is_opened(s.qualified))
                    .count();
                ui.horizontal(|ui| {
                    ui.heading(egui::RichText::new("Learning paths").size(16.0));
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} of {} steps opened",
                            done_steps, total_steps
                        ))
                        .size(11.0)
                        .color(muted),
                    );
                });
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(
                        "Three guided tours through Modelica. Click to \
                         expand, click a step to open it read-only.",
                    )
                    .size(11.0)
                    .color(muted),
                );
                ui.add_space(10.0);

                for (i, path) in PATHS.iter().enumerate() {
                    let opened = path
                        .steps
                        .iter()
                        .filter(|s| progress.is_opened(s.qualified))
                        .count();
                    let is_expanded = wstate.expanded == Some(i);

                    // Header button — full-width, title row + dots.
                    let header_h = 56.0;
                    let resp = ui
                        .add_sized(
                            [ui.available_width(), header_h],
                            egui::Button::new("")
                                .fill(card_fill)
                                .stroke(egui::Stroke::new(1.0, card_stroke)),
                        )
                        .on_hover_text(if is_expanded {
                            "Click to collapse".to_string()
                        } else {
                            format!(
                                "Click to expand ({} of {} opened)",
                                opened,
                                path.steps.len()
                            )
                        });
                    let rect = resp.rect;
                    let painter = ui.painter_at(rect);

                    // Title row.
                    painter.text(
                        rect.min + egui::vec2(16.0, 10.0),
                        egui::Align2::LEFT_TOP,
                        format!("{}  {}", path.icon, path.title),
                        egui::FontId::proportional(15.0),
                        title_tint,
                    );
                    painter.text(
                        rect.min + egui::vec2(16.0, 32.0),
                        egui::Align2::LEFT_TOP,
                        path.subtitle,
                        egui::FontId::proportional(11.0),
                        muted,
                    );

                    // Progress dots — right-aligned. ✅ = opened,
                    // ○ = not yet. Plus a chevron hinting at
                    // expandable state.
                    let mut dots = String::new();
                    for step in path.steps {
                        dots.push_str(if progress.is_opened(step.qualified) {
                            "● "
                        } else {
                            "○ "
                        });
                    }
                    painter.text(
                        rect.max - egui::vec2(16.0, header_h - 14.0),
                        egui::Align2::RIGHT_TOP,
                        dots.trim_end(),
                        egui::FontId::proportional(14.0),
                        if opened > 0 { success } else { muted },
                    );
                    painter.text(
                        rect.max - egui::vec2(16.0, 22.0),
                        egui::Align2::RIGHT_TOP,
                        if is_expanded {
                            format!(
                                "{} of {}  ▾",
                                opened,
                                path.steps.len()
                            )
                        } else {
                            format!(
                                "{} of {}  ▸",
                                opened,
                                path.steps.len()
                            )
                        },
                        egui::FontId::proportional(11.0),
                        muted,
                    );

                    if resp.clicked() {
                        wstate.expanded =
                            if is_expanded { None } else { Some(i) };
                    }

                    ui.add_space(6.0);

                    // Expanded step list.
                    if is_expanded {
                        for step in path.steps {
                            let opens = progress.opens_of(step.qualified);
                            let is_open = opens > 0;

                            // Resolve index entry for path-exists
                            // validation (rare missing means label
                            // still renders but click is disabled).
                            let exists = msl_component_library()
                                .iter()
                                .any(|c| c.msl_path == step.qualified);

                            let step_h = 48.0;
                            let indent = 32.0;
                            let step_w = (ui.available_width() - indent).max(220.0);
                            ui.horizontal(|ui| {
                                ui.add_space(indent);
                                let resp = ui.add_enabled(
                                    exists,
                                    egui::Button::new("")
                                        .min_size(egui::vec2(step_w, step_h))
                                        .fill(card_fill)
                                        .stroke(egui::Stroke::new(
                                            1.0,
                                            card_stroke,
                                        )),
                                );
                                let resp = if exists {
                                    resp.on_hover_text(format!(
                                        "{}\n\n{}\n\n{}",
                                        step.qualified,
                                        step.goal,
                                        if opens == 0 {
                                            "Not opened yet.".to_string()
                                        } else if opens == 1 {
                                            "Opened once.".to_string()
                                        } else {
                                            format!("Opened {} times.", opens)
                                        },
                                    ))
                                } else {
                                    resp.on_hover_text(
                                        "Missing from msl_index.json — \
                                         re-run the indexer.",
                                    )
                                };
                                let rect = resp.rect;
                                let painter = ui.painter_at(rect);

                                // Status dot.
                                painter.text(
                                    rect.min + egui::vec2(14.0, 15.0),
                                    egui::Align2::LEFT_TOP,
                                    if is_open { "●" } else { "○" },
                                    egui::FontId::proportional(15.0),
                                    if is_open { success } else { muted },
                                );
                                // Name + goal.
                                painter.text(
                                    rect.min + egui::vec2(36.0, 8.0),
                                    egui::Align2::LEFT_TOP,
                                    step.label,
                                    egui::FontId::proportional(13.0),
                                    title_tint,
                                );
                                painter.text(
                                    rect.min + egui::vec2(36.0, 28.0),
                                    egui::Align2::LEFT_TOP,
                                    format!("Goal: {}", step.goal),
                                    egui::FontId::proportional(10.5),
                                    muted,
                                );
                                // Open-count pill on the right —
                                // only shown after first open so
                                // "1 run" doesn't clutter a fresh
                                // install.
                                if opens > 0 {
                                    painter.text(
                                        rect.max - egui::vec2(14.0, 28.0),
                                        egui::Align2::RIGHT_TOP,
                                        format!(
                                            "{} open{}",
                                            opens,
                                            if opens == 1 { "" } else { "s" }
                                        ),
                                        egui::FontId::proportional(10.5),
                                        success,
                                    );
                                }

                                if exists && resp.clicked() {
                                    open_msl = Some(step.qualified.to_string());
                                }
                            });
                            ui.add_space(4.0);
                        }
                        ui.add_space(8.0);
                    }
                }
            });

            ui.add_space(24.0);

            // ── Browse all examples (collapsed by default) ─
            ui.vertical_centered(|ui| {
                // Adaptive width — honour 760px as the comfort-read
                // ceiling but shrink to fit narrow panels so the
                // left edge doesn't clip under the dock rail.
                let w = ui.available_width().min(760.0);
                ui.set_max_width(w);
                let lib = msl_component_library();
                let examples: Vec<&MSLComponentDef> =
                    lib.iter().filter(|c| is_top_level_example(c)).collect();

                egui::CollapsingHeader::new(
                    egui::RichText::new(format!(
                        "Browse all {} examples",
                        examples.len()
                    ))
                    .size(14.0)
                    .color(title_tint),
                )
                .id_salt("welcome_browse_all")
                .default_open(false)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(
                            "The full Modelica Standard Library example set. \
                             Filter by domain; search across name or description.",
                        )
                        .size(11.0)
                        .color(muted),
                    );
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.label("🔍");
                        let _ = ui.add_sized(
                            [560.0, 26.0],
                            egui::TextEdit::singleline(&mut wstate.browse_query)
                                .hint_text("search…"),
                        );
                        if !wstate.browse_query.is_empty()
                            && ui
                                .button("✕")
                                .on_hover_text("Clear search")
                                .clicked()
                        {
                            wstate.browse_query.clear();
                        }
                    });

                    ui.add_space(6.0);

                    // Domain chips — same as before but compact.
                    let mut domain_counts: Vec<(String, usize)> = {
                        let mut map: std::collections::HashMap<
                            String,
                            usize,
                        > = std::collections::HashMap::new();
                        for c in &examples {
                            *map.entry(c.domain.clone()).or_default() += 1;
                        }
                        map.into_iter().collect()
                    };
                    domain_counts
                        .sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

                    ui.horizontal_wrapped(|ui| {
                        let mut chip =
                            |ui: &mut egui::Ui,
                             label: String,
                             active: bool|
                             -> egui::Response {
                                let (fill, fg) = if active {
                                    (chip_fill_active, chip_text_active)
                                } else {
                                    (chip_fill_idle, chip_text_idle)
                                };
                                ui.add(
                                    egui::Button::new(
                                        egui::RichText::new(label)
                                            .size(11.0)
                                            .color(fg),
                                    )
                                    .fill(fill)
                                    .stroke(egui::Stroke::new(
                                        1.0,
                                        chip_fill_idle,
                                    )),
                                )
                            };

                        if chip(
                            ui,
                            format!("All ({})", examples.len()),
                            wstate.browse_domain.is_empty(),
                        )
                        .clicked()
                        {
                            wstate.browse_domain.clear();
                        }
                        for (domain, count) in &domain_counts {
                            let label = format!(
                                "{} {} ({})",
                                domain_icon(domain),
                                domain,
                                count
                            );
                            if chip(
                                ui,
                                label,
                                wstate.browse_domain == *domain,
                            )
                            .clicked()
                            {
                                wstate.browse_domain = domain.clone();
                            }
                        }
                    });

                    ui.add_space(8.0);

                    let query_lc = wstate.browse_query.to_lowercase();
                    let filtered: Vec<&MSLComponentDef> = examples
                        .iter()
                        .copied()
                        .filter(|c| {
                            (wstate.browse_domain.is_empty()
                                || c.domain == wstate.browse_domain)
                                && (query_lc.is_empty()
                                    || c.name.to_lowercase().contains(&query_lc)
                                    || c.msl_path
                                        .to_lowercase()
                                        .contains(&query_lc)
                                    || c.short_description
                                        .as_deref()
                                        .is_some_and(|s| {
                                            s.to_lowercase().contains(&query_lc)
                                        })
                                    || c.documentation_info
                                        .as_deref()
                                        .is_some_and(|s| {
                                            s.to_lowercase().contains(&query_lc)
                                        }))
                        })
                        .collect();

                    if filtered.is_empty() {
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new("No examples match.")
                                .color(muted),
                        );
                    } else {
                        let col_w = 372.0;
                        let row_h = 62.0;
                        let mut iter = filtered.iter();
                        loop {
                            let left = iter.next();
                            let right = iter.next();
                            if left.is_none() {
                                break;
                            }
                            ui.horizontal(|ui| {
                                for entry in [left, right].into_iter().flatten()
                                {
                                    let c = *entry;
                                    let resp = ui
                                        .add_sized(
                                            [col_w, row_h],
                                            egui::Button::new("")
                                                .fill(card_fill)
                                                .stroke(egui::Stroke::new(
                                                    1.0,
                                                    card_stroke,
                                                )),
                                        )
                                        .on_hover_text(format!(
                                            "{}\n\nOpens read-only — \
                                             duplicate to edit.",
                                            c.msl_path
                                        ));
                                    let rect = resp.rect;
                                    let painter = ui.painter_at(rect);
                                    let dot = if progress.is_opened(&c.msl_path)
                                    {
                                        ("●", success)
                                    } else {
                                        ("○", muted)
                                    };
                                    painter.text(
                                        rect.min + egui::vec2(12.0, 10.0),
                                        egui::Align2::LEFT_TOP,
                                        dot.0,
                                        egui::FontId::proportional(13.0),
                                        dot.1,
                                    );
                                    painter.text(
                                        rect.min + egui::vec2(28.0, 8.0),
                                        egui::Align2::LEFT_TOP,
                                        format!(
                                            "{}  {}",
                                            domain_icon(&c.domain),
                                            c.name
                                        ),
                                        egui::FontId::proportional(13.0),
                                        title_tint,
                                    );
                                    let sub = card_subtitle(c);
                                    let sub = if sub.chars().count() > 72 {
                                        let mut s: String =
                                            sub.chars().take(72).collect();
                                        s.push('…');
                                        s
                                    } else {
                                        sub
                                    };
                                    painter.text(
                                        rect.min + egui::vec2(28.0, 28.0),
                                        egui::Align2::LEFT_TOP,
                                        sub,
                                        egui::FontId::proportional(10.5),
                                        muted,
                                    );
                                    painter.text(
                                        rect.min
                                            + egui::vec2(28.0, row_h - 16.0),
                                        egui::Align2::LEFT_TOP,
                                        &c.domain,
                                        egui::FontId::proportional(9.5),
                                        muted,
                                    );
                                    if resp.clicked() {
                                        open_msl =
                                            Some(c.msl_path.clone());
                                    }
                                }
                            });
                            ui.add_space(6.0);
                        }
                    }
                });
            });

            ui.add_space(24.0);

            // ── Shortcuts footer ───────────────────────────
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(
                        "Ctrl+N  new    ·    Ctrl+S  save    ·    \
                         Ctrl+Z / Ctrl+Shift+Z  undo/redo    ·    \
                         F5  compile",
                    )
                    .size(10.0)
                    .color(egui::Color32::DARK_GRAY),
                );
            });

            ui.add_space(32.0);
        });

        // Persist per-panel UI state.
        ui.ctx().data_mut(|d| d.insert_temp(state_id, wstate));

        // ── Side effects ──────────────────────────────────
        if create_new {
            world
                .commands()
                .trigger(crate::ui::commands::CreateNewScratchModel);
        }
        if open_folder {
            if let Some(folder) = rfd::FileDialog::new()
                .set_title("Open workspace folder")
                .pick_folder()
            {
                use bevy::tasks::AsyncComputeTaskPool;
                let pool = AsyncComputeTaskPool::get();
                let task = pool.spawn({
                    let folder = folder.clone();
                    async move {
                        crate::ui::panels::package_browser::scan_twin_folder(
                            folder,
                        )
                    }
                });
                {
                    let mut cache = world.resource_mut::<
                        crate::ui::panels::package_browser::PackageTreeCache,
                    >();
                    cache.twin = None;
                    cache.twin_scan_task = Some(task);
                }
                match lunco_twin::TwinMode::open(&folder) {
                    Ok(lunco_twin::TwinMode::Folder(twin))
                    | Ok(lunco_twin::TwinMode::Twin(twin)) => {
                        let twin_id = world
                            .resource_mut::<lunco_workbench::WorkspaceResource>(
                            )
                            .add_twin(twin);
                        world
                            .commands()
                            .trigger(lunco_workbench::TwinAdded {
                                twin: twin_id,
                            });
                    }
                    Ok(lunco_twin::TwinMode::Orphan(_)) => {}
                    Err(e) => {
                        log::warn!(
                            "open folder: failed to index {:?}: {}",
                            folder,
                            e
                        );
                    }
                }
            }
        }
        if let Some(filename) = open_bundled {
            let id = format!("bundled://{}", filename);
            let name = filename
                .strip_suffix(".mo")
                .unwrap_or(filename)
                .to_string();
            crate::ui::panels::package_browser::open_model(
                world,
                id,
                name,
                ModelLibrary::Bundled,
            );
        }
        if let Some(qualified) = open_msl {
            world
                .commands()
                .trigger(crate::ui::commands::OpenClass { qualified });
        }
    }
}
