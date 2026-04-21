//! `line_plot` — 2D time-series line chart.
//!
//! The workhorse viz kind: one or more scalar signals drawn against
//! time. Role `"y"` accepts any number of `SignalType::Scalar` bindings;
//! each binding becomes one line. X axis is always the signal's
//! `time` field.
//!
//! Feature parity with the pre-refactor Modelica Graphs panel. Legend
//! is click-to-toggle (egui_plot builtin), Auto-Fit is driven from the
//! panel toolbar (see `VizPanel`).

use bevy::prelude::*;
use bevy_egui::egui;
use egui_plot::{Corner, Legend, Line, Plot, PlotPoints};

use crate::registry::VizFitRequests;
use crate::signal::{SignalRegistry, SignalType};
use crate::view::{Panel2DCtx, ViewKind};
use crate::viz::{RoleSpec, SignalBinding, VisualizationConfig, Visualization, VizKindId};

pub const LINE_PLOT_KIND: VizKindId = VizKindId::new_static("line_plot");

const ROLE_Y: RoleSpec = RoleSpec {
    role: "y",
    accepted_types: &[SignalType::Scalar],
    single: false,
};

/// Default palette for auto-assigned colors. Each binding without an
/// explicit `color` picks the next slot round-robin. Same palette as
/// the old Graphs panel for visual continuity.
const PALETTE: &[egui::Color32] = &[
    egui::Color32::from_rgb(80, 160, 255),
    egui::Color32::from_rgb(255, 120, 80),
    egui::Color32::from_rgb(80, 220, 120),
    egui::Color32::from_rgb(255, 220, 80),
    egui::Color32::from_rgb(200, 120, 255),
    egui::Color32::from_rgb(120, 200, 200),
    egui::Color32::from_rgb(230, 120, 180),
    egui::Color32::from_rgb(180, 230, 100),
];

#[derive(Default)]
pub struct LinePlot;

impl Visualization for LinePlot {
    fn kind_id(&self) -> VizKindId { LINE_PLOT_KIND.clone() }
    fn display_name(&self) -> &'static str { "Line plot (time-series)" }
    fn role_schema(&self) -> &'static [RoleSpec] { &[ROLE_Y] }
    fn compatible_views(&self) -> &'static [ViewKind] { &[ViewKind::Panel2D] }

    fn render_panel_2d(&self, ctx: &mut Panel2DCtx, config: &VisualizationConfig) {
        let registry = match ctx.world.get_resource::<SignalRegistry>() {
            Some(r) => r,
            None => {
                ctx.ui.label("SignalRegistry not installed.");
                return;
            }
        };

        // Collect `y`-role bindings. Filter hidden + missing-signal
        // bindings here so the legend shows the same set as the plot.
        let y_bindings: Vec<&SignalBinding> = config
            .inputs
            .iter()
            .filter(|b| b.role == ROLE_Y.role && b.visible)
            .collect();

        if y_bindings.is_empty() {
            ctx.ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                ui.label(
                    egui::RichText::new("No signals bound.")
                        .color(egui::Color32::GRAY),
                );
                ui.label(
                    egui::RichText::new(
                        "Drag a variable from Telemetry or use the inspector.",
                    )
                    .size(10.0)
                    .color(egui::Color32::DARK_GRAY),
                );
            });
            return;
        }

        // Build the egui_plot `Line`s up-front so we can release the
        // registry borrow before calling `plot.show()` (which wants a
        // long-lived borrow on `ctx.ui`).
        let lines: Vec<Line> = y_bindings
            .iter()
            .enumerate()
            .filter_map(|(i, b)| {
                let hist = registry.scalar_history(&b.source)?;
                if hist.is_empty() {
                    return None;
                }
                let pts: Vec<[f64; 2]> =
                    hist.iter().map(|s| [s.time, s.value]).collect();
                let label = b.label.clone().unwrap_or_else(|| b.source.path.clone());
                // Universal palette: same signal path → same colour
                // everywhere. Per-binding `color` override still wins.
                // The local `PALETTE` round-robin is the legacy
                // fallback when the signal path is empty (rare).
                let _ = i;
                let color = b.color.unwrap_or_else(|| {
                    if b.source.path.is_empty() {
                        PALETTE[0]
                    } else {
                        crate::signal::color_for_signal(&b.source.path)
                    }
                });
                Some(Line::new(label, PlotPoints::new(pts)).color(color))
            })
            .collect();

        // Consume any pending Fit request for this viz. `auto_bounds`
        // alone only controls the *initial* policy — once the user
        // pans or zooms, egui_plot remembers their view and ignores
        // a policy change. `Plot::reset()` forces the plot to
        // discard stored memory and re-fit to the data exactly once,
        // which is what the user expects from a Fit button.
        let fit_requested = ctx
            .world
            .get_resource_mut::<VizFitRequests>()
            .map(|mut r| r.take(config.id))
            .unwrap_or(false);

        // Let the plot fill its tile. Explicit `view_aspect` would
        // distort a time-series (X in seconds, Y in arbitrary units
        // — not commensurable); `include_y(0.0)` previously compressed
        // all dynamics against zero for signals offset far from it.
        // Both dropped: we let egui_plot's default auto-fit pick a
        // tight range around the data on the first draw.
        let tile_rect = ctx.ui.max_rect();
        let mut plot = Plot::new(("line_plot", config.id.raw()))
            .width(tile_rect.width())
            .height(tile_rect.height())
            .auto_bounds(bevy_egui::egui::emath::Vec2b::new(true, true))
            .legend(
                Legend::default()
                    .position(Corner::RightTop)
                    .background_alpha(0.7),
            );
        if fit_requested {
            plot = plot.reset();
        }

        plot.show(ctx.ui, |plot_ui| {
            for line in lines {
                plot_ui.line(line);
            }
        });
    }
}
