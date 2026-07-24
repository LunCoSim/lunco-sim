//! Perspective-based help system.
//!
//! Human-authored quick-reference for each [`Perspective`]: title,
//! description, keyboard shortcuts, mouse interactions. Surfaced as a
//! **modal popup** (not a dock panel) so it reads as a transient "what
//! can I do here" overlay rather than another tab to manage.
//!
//! The Help menu gets **one entry per registered perspective**
//! (`📖 <Perspective> Help`); each opens a popup showing just that
//! perspective's controls. Closed with Esc or by clicking the dimmed
//! backdrop.

pub(crate) use crate::{PerspectiveId, WorkbenchLayout};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use std::collections::HashMap;

/// A single keyboard shortcut entry.
#[derive(Debug, Clone, Default)]
pub struct HelpShortcut {
    /// The key combination (e.g. "Ctrl+S", "F5").
    pub keys: &'static str,
    /// What it does.
    pub description: &'static str,
}

/// A single mouse interaction entry.
#[derive(Debug, Clone, Default)]
pub struct HelpMouse {
    /// The input action (e.g. "Right Drag", "Scroll").
    pub interaction: &'static str,
    /// What it does.
    pub description: &'static str,
}

/// Human-authored help content for a Perspective.
#[derive(Debug, Clone, Default)]
pub struct PerspectiveHelp {
    /// Human-readable title of the perspective.
    pub title: &'static str,
    /// One-paragraph summary of what this perspective is for.
    pub description: &'static str,
    /// Primary keyboard shortcuts.
    pub shortcuts: Vec<HelpShortcut>,
    /// Primary mouse interactions.
    pub mouse: Vec<HelpMouse>,
    /// Whether this perspective has a guided tour. When set, the popup
    /// shows a "🎓 Show Tour" button that publishes a [`HelpTourRequest`]
    /// for this perspective; the owning domain observes it and starts
    /// its tour (the workbench has no tour of its own).
    pub has_tour: bool,
}

/// Registry of help content for all perspectives in the app.
#[derive(Resource, Default)]
pub struct PerspectiveHelpRegistry {
    pub(crate) entries: HashMap<PerspectiveId, PerspectiveHelp>,
}

impl PerspectiveHelpRegistry {
    /// Register help for a perspective.
    pub fn register(&mut self, id: PerspectiveId, help: PerspectiveHelp) {
        self.entries.insert(id, help);
    }

    /// Get help for a perspective, if any.
    pub fn get(&self, id: PerspectiveId) -> Option<&PerspectiveHelp> {
        self.entries.get(&id)
    }
}

/// Which perspective's help popup is open, if any.
#[derive(Resource, Default)]
pub struct HelpPopup(pub Option<PerspectiveId>);

/// Set by the popup's "Show Tour" button to the perspective whose tour
/// was requested. The domain that owns the tour drains it (sets it back
/// to `None`) when it starts. Decouples the workbench-level popup from
/// domain-specific tour code.
#[derive(Resource, Default)]
pub struct HelpTourRequest(pub Option<PerspectiveId>);

/// Plugin that adds the perspective help system, registry, and popup.
pub struct PerspectiveHelpPlugin;

impl Plugin for PerspectiveHelpPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PerspectiveHelpRegistry>();
        app.init_resource::<HelpPopup>();
        app.init_resource::<HelpTourRequest>();
        app.add_systems(
            EguiPrimaryContextPass,
            render_help_popup.after(crate::WorkbenchRenderSet),
        );
    }
}

/// Add a Help-menu entry that opens `id`'s help popup. Called by
/// [`WorkbenchAppExt::register_perspective_help`](crate::WorkbenchAppExt)
/// so each subsystem contributes its *own* menu item at the point it
/// registers its perspective — the workbench never hardcodes a list.
pub(crate) fn register_help_menu_item(layout: &mut WorkbenchLayout, id: PerspectiveId) {
    layout.register_help_menu(move |ui, world, _layout| {
        let label = world
            .resource::<PerspectiveHelpRegistry>()
            .get(id)
            .map(|h| format!("📖 {} Help", h.title));
        if let Some(label) = label {
            if ui.button(label).clicked() {
                world.resource_mut::<HelpPopup>().0 = Some(id);
                ui.close();
            }
        }
    });
}

/// Renders one help row: a fixed-width left column (key / interaction)
/// and a wrapping right column (description) that fills the remaining
/// horizontal space so long text never clips.
fn help_row(ui: &mut egui::Ui, left: egui::RichText, right: &str, right_color: egui::Color32) {
    const KEY_COL_W: f32 = 150.0;
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(KEY_COL_W, 0.0),
            egui::Layout::left_to_right(egui::Align::Min),
            |ui| {
                ui.label(left);
            },
        );
        ui.add(egui::Label::new(egui::RichText::new(right).color(right_color)).wrap());
    });
    ui.add_space(5.0);
}

fn render_help_popup(
    mut egui_ctx: EguiContexts,
    mut popup: ResMut<HelpPopup>,
    mut tour_req: ResMut<HelpTourRequest>,
    registry: Res<PerspectiveHelpRegistry>,
    theme: Option<Res<lunco_theme::Theme>>,
) {
    let Some(id) = popup.0 else {
        return;
    };
    let Ok(ctx) = egui_ctx.ctx_mut() else {
        return;
    };
    let Some(help) = registry.get(id) else {
        popup.0 = None;
        return;
    };

    let theme = theme
        .map(|t| t.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);
    let accent = theme.tokens.accent;
    let muted = theme.tokens.text_subdued;
    let text = theme.colors.text;
    let surface_raised = theme.tokens.surface_raised;

    let viewport = ctx.content_rect();
    let mut close = false;

    // ── Dimmed backdrop — click anywhere outside the card to close ──
    egui::Area::new(egui::Id::new("perspective_help_scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(viewport.min)
        .interactable(true)
        .show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(viewport.size(), egui::Sense::click());
            ui.painter().rect_filled(rect, 0.0, theme.tokens.scrim);
            if resp.clicked() {
                close = true;
            }
        });

    // ── Card ────────────────────────────────────────────────────────
    // Wide card so the description column has room to breathe; scales
    // with the window but capped so it never spans an ultrawide display.
    let card_w = (viewport.width() * 0.9).clamp(440.0, 720.0);
    let card_fill = {
        let [r, g, b, _] = surface_raised.to_array();
        egui::Color32::from_rgba_unmultiplied(r, g, b, 252)
    };

    egui::Area::new(egui::Id::new("perspective_help_card"))
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .interactable(true)
        .show(ctx, |ui| {
            ui.set_width(card_w);
            egui::Frame::new()
                .fill(card_fill)
                .corner_radius(14.0)
                .inner_margin(egui::Margin::symmetric(20, 18))
                .stroke(egui::Stroke::new(1.5, accent))
                .show(ui, |ui| {
                    // Header — title + close button on the same row.
                    ui.horizontal(|ui| {
                        ui.heading(egui::RichText::new(help.title).color(text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✖").on_hover_text("Close (Esc)").clicked() {
                                close = true;
                            }
                        });
                    });
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new(help.description).color(muted));
                    ui.add_space(14.0);

                    egui::ScrollArea::vertical()
                        .max_height(viewport.height() * 0.66)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if !help.shortcuts.is_empty() {
                                ui.strong("⌨ Keyboard Shortcuts");
                                ui.add_space(6.0);
                                for s in &help.shortcuts {
                                    help_row(
                                        ui,
                                        egui::RichText::new(s.keys).code().strong(),
                                        s.description,
                                        text,
                                    );
                                }
                                ui.add_space(12.0);
                            }

                            if !help.mouse.is_empty() {
                                ui.strong("🖱 Mouse Controls");
                                ui.add_space(6.0);
                                for m in &help.mouse {
                                    help_row(
                                        ui,
                                        egui::RichText::new(m.interaction).italics(),
                                        m.description,
                                        text,
                                    );
                                }
                            }

                            if help.shortcuts.is_empty() && help.mouse.is_empty() {
                                ui.label(egui::RichText::new("No shortcuts listed.").weak());
                            }
                        });

                    // Guided-tour launcher — only for perspectives that
                    // declared one. Lives below the scroll area so it's
                    // always reachable.
                    if help.has_tour {
                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(8.0);
                        if ui
                            .button(egui::RichText::new("🎓 Show Tour").strong())
                            .on_hover_text("Replay the guided interactive tour")
                            .clicked()
                        {
                            tour_req.0 = Some(id);
                            close = true;
                        }
                    }
                });
        });

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if close {
        popup.0 = None;
    }
}
