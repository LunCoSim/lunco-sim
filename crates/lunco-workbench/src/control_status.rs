//! The blackout badge: "commands will not reach this vessel".
//!
//! Reads [`ControlPathRegistry`] — the SAME fact the authorization gate reads when
//! it refuses a drive command. That is the whole point of sourcing it here rather
//! than from a separate status flag: the indicator cannot disagree with the
//! refusal, because there is only one fact. A student who sees the badge and cannot
//! drive is seeing the cause, not a coincidence.
//!
//! ## Why not `SubsystemToggles`
//!
//! `comms-degradation` looks like the obvious source and is the wrong one, twice
//! over. It is a **progressive-fidelity switch** — "this lesson now simulates comms
//! degradation" (see `lunco-core/src/subsystems.rs`) — not a live state, and
//! `SubsystemToggles::enabled` **defaults to `true` for an unset key**, so a scene
//! that never mentions comms would render as permanently blacked out. Space School
//! was flipping it per-tick as though it meant "the link is down right now"; that
//! misuse is gone.
//!
//! ## Not comms
//!
//! Nothing here says radio. A blackout is a blackout whether it comes from terrain
//! occlusion, a jammer, a dead receiver or an OBC fault — the badge reports that
//! commands do not arrive, which is the fact the student needs and the only one the
//! core actually knows (doc 49 §1).

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use lunco_core::session::ControlPathRegistry;
use lunco_core::GlobalEntityId;

/// Draws the control-blackout badge described in the module docs.
///
/// Adds a single `Update` system; holds no state of its own, because the badge
/// is derived from [`ControlPathRegistry`] each frame rather than cached — that
/// is what keeps it unable to disagree with the authorization gate.
pub struct ControlStatusPlugin;

impl Plugin for ControlStatusPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, draw_control_blackout);
    }
}

/// One badge per vessel whose control path is down. Draws nothing — costs nothing —
/// when no mission has declared a blackout, which is every scene by default.
fn draw_control_blackout(
    mut egui_ctx: EguiContexts,
    paths: Option<Res<ControlPathRegistry>>,
    q: Query<(&GlobalEntityId, Option<&Name>)>,
    theme: Option<Res<lunco_theme::Theme>>,
) {
    let Some(paths) = paths else { return };
    let theme = theme
        .map(|t| t.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);
    let alert = theme.tokens.error;

    // Name the affected vessels. Cheap: the query is only walked when something is
    // actually down, and `is_down` is a hash lookup.
    let mut down: Vec<String> = q
        .iter()
        .filter(|(gid, _)| paths.is_down(gid.get()))
        .map(|(gid, name)| {
            name.map(|n| leaf(n.as_str()).to_string())
                .unwrap_or_else(|| format!("vessel {}", gid.get()))
        })
        .collect();
    if down.is_empty() {
        return;
    }
    down.sort();
    down.dedup();

    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("lunco_control_blackout"))
        .order(egui::Order::Foreground)
        .interactable(false)
        // Top-centre: this is the one thing on screen that explains why the
        // controls stopped answering, so it does not go in a corner.
        .fixed_pos(egui::pos2(screen.center().x - 150.0, screen.top() + 44.0))
        .show(ctx, |ui| {
            ui.set_max_width(300.0);
            egui::Frame::new()
                // TODO(theme): migrate to lunco-theme once the token set covers this.
                // Error-tinted backdrop for a HUD panel that is itself the alarm —
                // `overlay_backdrop` is neutral and `error_subdued` is a chip fill,
                // so neither is right; this wants an alert-backdrop token.
                .fill(egui::Color32::from_rgba_unmultiplied(46, 18, 22, 235))
                .corner_radius(10.0)
                .stroke(egui::Stroke::new(1.0, alert.linear_multiply(0.7)))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("⚠  NO LINK")
                                .color(alert)
                                .size(16.0)
                                .strong(),
                        );
                        // Say what it MEANS, not just that it happened: the lesson is
                        // that the rover is on its own, not that it is broken.
                        ui.label(
                            egui::RichText::new(format!(
                                "commands are not reaching {} — autonomy only",
                                down.join(", ")
                            ))
                            .color(theme.tokens.text)
                            .size(13.0),
                        );
                    });
                });
        });
}

/// Prim paths name entities (`/Traverse/Rover`); a student wants "Rover".
fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}
