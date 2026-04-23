//! `line_plot` — 2D line chart.
//!
//! Role `"y"` accepts any number of `SignalType::Scalar` bindings;
//! each binding becomes one line. The X axis is **time by default**
//! but can be swapped to any bound signal to produce a phase-space
//! trajectory (`phi_rel` vs `w_rel`, etc.).
//!
//! Each plot panel has its **own** config — the `+ New Plot` button
//! creates a fresh `VizId` and a matching `VisualizationConfig`. A
//! small per-panel toolbar exposes an X-axis picker, the current
//! Y-binding chips (each with ×), and an "+ Add signal" dropdown
//! populated from the `SignalRegistry`. Without this toolbar, new
//! plots appeared frozen to users because the only signal-picker
//! (Telemetry's checkboxes) targeted the default plot exclusively.
//!
//! Style is stored in `VisualizationConfig.style` as
//! [`LinePlotStyle`] (serde JSON) so the choice survives save/reload.

use bevy::prelude::*;
use bevy_egui::egui;
use egui_plot::{Corner, Legend, Line, Plot, PlotPoints};
use serde::{Deserialize, Serialize};

use crate::registry::{VisualizationRegistry, VizFitRequests};
use crate::signal::{SignalRegistry, SignalRef, SignalType, ScalarSample};
use crate::view::{Panel2DCtx, ViewKind};
use crate::viz::{RoleSpec, SignalBinding, VisualizationConfig, Visualization, VizKindId};

/// LinePlot-specific options stashed in
/// [`VisualizationConfig::style`]. Serialised as JSON for on-disk
/// round-trip. All fields optional — missing fields keep default
/// behaviour (X = time, auto-labeled axes).
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct LinePlotStyle {
    /// Signal to drive the X axis. `None` = simulation time (the
    /// classic time-series plot). When `Some`, each Y sample is
    /// paired with the X sample at the same (or nearest-earlier)
    /// time, producing a phase-space trajectory.
    #[serde(default)]
    pub x_signal: Option<SignalRef>,
    /// Optional axis labels. If `None`, labels are auto-derived
    /// from the X-signal path (or `"time (s)"`) and the first Y
    /// binding's path.
    #[serde(default)]
    pub x_label: Option<String>,
    #[serde(default)]
    pub y_label: Option<String>,
}

impl LinePlotStyle {
    fn load(config: &VisualizationConfig) -> Self {
        serde_json::from_value(config.style.clone()).unwrap_or_default()
    }
    fn save(&self, config: &mut VisualizationConfig) {
        config.style = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
    }
}

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
        // Toolbar first — lets the user edit bindings even before
        // any signal data arrives. Returns the mutation the user
        // requested, which we apply after releasing the read borrow
        // on the registry.
        let edit = render_toolbar(ctx, config);
        if let Some(edit) = edit {
            apply_edit(ctx.world, config.id, edit);
            // Don't render the body this frame — the config just
            // changed. Next frame picks up the new bindings.
            return;
        }

        let style = LinePlotStyle::load(config);
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
                        "Add one from the ➕ picker above, or drag a \
                         variable from Telemetry.",
                    )
                    .size(10.0)
                    .color(egui::Color32::DARK_GRAY),
                );
            });
            return;
        }

        // Resolve the X-axis source once. For classic time-series we
        // just use each Y sample's own `time`. For phase-space mode
        // we pull the X signal's history and pair by time below.
        let x_samples: Option<Vec<ScalarSample>> = style.x_signal.as_ref().and_then(|xs| {
            registry
                .scalar_history(xs)
                .filter(|h| !h.is_empty())
                .map(|h| h.iter().copied().collect())
        });

        // Build the egui_plot `Line`s up-front so we can release the
        // registry borrow before calling `plot.show()` (which wants a
        // long-lived borrow on `ctx.ui`).
        let lines: Vec<Line> = y_bindings
            .iter()
            .filter_map(|b| {
                let hist = registry.scalar_history(&b.source)?;
                if hist.is_empty() {
                    return None;
                }
                let pts: Vec<[f64; 2]> = match &x_samples {
                    None => {
                        // Classic time on X. Each sample's own `time`
                        // is its X coordinate.
                        hist.iter().map(|s| [s.time, s.value]).collect()
                    }
                    Some(xs) => pair_by_time(xs, hist.iter().copied()),
                };
                if pts.is_empty() {
                    return None;
                }
                // Legend label: explicit binding label wins; otherwise
                // start with the signal path and, when
                // `SignalRegistry` has a human description for the
                // variable, append it in parens so the legend
                // doubles as documentation. (egui_plot's `Legend`
                // has no per-item hover hook — inlining the
                // description text is the only way to surface it.)
                let label = b.label.clone().unwrap_or_else(|| {
                    match registry.meta(&b.source).and_then(|m| m.description.as_deref()) {
                        Some(desc) if !desc.trim().is_empty() => {
                            format!("{} ({})", b.source.path, desc.trim())
                        }
                        _ => b.source.path.clone(),
                    }
                });
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
        // discard stored memory and re-fit to the data exactly once.
        let fit_requested = ctx
            .world
            .get_resource_mut::<VizFitRequests>()
            .map(|mut r| r.take(config.id))
            .unwrap_or(false);

        // Axis labels. Explicit user override → that; otherwise
        // derive from the signal (X = signal path / "time (s)"; Y =
        // first binding path when only one Y is bound, else
        // "(see legend)" so the plot doesn't mislabel a multi-line
        // chart with the first signal's name).
        let x_label = style.x_label.clone().unwrap_or_else(|| match &style.x_signal {
            None => "time (s)".to_string(),
            Some(xs) => xs.path.clone(),
        });
        let y_label = style.y_label.clone().unwrap_or_else(|| {
            if y_bindings.len() == 1 {
                y_bindings[0].source.path.clone()
            } else {
                "(see legend)".to_string()
            }
        });

        // Use the space *remaining* after the toolbar + separator
        // above; `max_rect()` would double-count that strip and push
        // the plot off the bottom of the tile.
        let remaining = ctx.ui.available_size_before_wrap();
        let mut plot = Plot::new(("line_plot", config.id.raw()))
            .width(remaining.x)
            .height(remaining.y)
            .x_axis_label(x_label)
            .y_axis_label(y_label)
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

// ── Per-plot toolbar + editing ──────────────────────────────────────

/// The mutation the toolbar asked for, applied in a second pass so
/// we don't hold `&VisualizationConfig` while mutating the registry.
enum Edit {
    SetX(Option<SignalRef>),
    AddY(SignalRef),
    RemoveY(SignalRef),
}

fn render_toolbar(ctx: &mut Panel2DCtx, config: &VisualizationConfig) -> Option<Edit> {
    // Snapshot available signals + current style so we can render
    // without holding a long-lived registry borrow.
    let (available, current_y_paths): (Vec<SignalRef>, std::collections::HashSet<SignalRef>) = {
        let registry = ctx.world.get_resource::<SignalRegistry>();
        let available: Vec<SignalRef> = registry
            .map(|r| {
                r.iter_signals()
                    .filter_map(|(s, t)| (t == SignalType::Scalar).then(|| s.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let current: std::collections::HashSet<SignalRef> = config
            .inputs
            .iter()
            .filter(|b| b.role == ROLE_Y.role)
            .map(|b| b.source.clone())
            .collect();
        (available, current)
    };
    let style = LinePlotStyle::load(config);

    let mut edit: Option<Edit> = None;
    ctx.ui.horizontal_wrapped(|ui| {
        // X picker.
        ui.label(egui::RichText::new("X:").size(11.0));
        let x_current = style
            .x_signal
            .as_ref()
            .map(|s| s.path.clone())
            .unwrap_or_else(|| "time".to_string());
        egui::ComboBox::from_id_salt(("lp_x", config.id.raw()))
            .selected_text(x_current)
            .width(140.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(style.x_signal.is_none(), "time")
                    .clicked()
                    && style.x_signal.is_some()
                {
                    edit = Some(Edit::SetX(None));
                }
                for sig in &available {
                    let selected = style.x_signal.as_ref() == Some(sig);
                    if ui.selectable_label(selected, &sig.path).clicked()
                        && !selected
                    {
                        edit = Some(Edit::SetX(Some(sig.clone())));
                    }
                }
            });
        ui.separator();

        // Y chips.
        ui.label(egui::RichText::new("Y:").size(11.0));
        let mut removed: Option<SignalRef> = None;
        for b in config.inputs.iter().filter(|b| b.role == ROLE_Y.role) {
            let chip = ui
                .small_button(format!("{} ✕", b.source.path))
                .on_hover_text("Remove from this plot");
            if chip.clicked() {
                removed = Some(b.source.clone());
            }
        }
        if let Some(r) = removed {
            edit = Some(Edit::RemoveY(r));
        }

        // Y add.
        let addables: Vec<&SignalRef> = available
            .iter()
            .filter(|s| !current_y_paths.contains(s))
            .collect();
        if !addables.is_empty() {
            egui::ComboBox::from_id_salt(("lp_add", config.id.raw()))
                .selected_text("➕ add")
                .width(120.0)
                .show_ui(ui, |ui| {
                    for sig in addables {
                        if ui.button(&sig.path).clicked() {
                            edit = Some(Edit::AddY(sig.clone()));
                        }
                    }
                });
        } else if current_y_paths.is_empty() {
            ui.label(
                egui::RichText::new("no signals yet")
                    .color(egui::Color32::DARK_GRAY)
                    .size(10.0),
            );
        }
    });
    ctx.ui.separator();
    edit
}

fn apply_edit(world: &mut World, viz: crate::viz::VizId, edit: Edit) {
    let Some(mut registry) = world.get_resource_mut::<VisualizationRegistry>() else {
        return;
    };
    let Some(cfg) = registry.get_mut(viz) else {
        return;
    };
    match edit {
        Edit::SetX(new) => {
            let mut style = LinePlotStyle::load(cfg);
            style.x_signal = new;
            style.save(cfg);
        }
        Edit::AddY(sig) => {
            if !cfg.inputs.iter().any(|b| b.source == sig) {
                cfg.inputs.push(SignalBinding {
                    source: sig,
                    role: ROLE_Y.role.to_string(),
                    label: None,
                    color: None,
                    visible: true,
                });
            }
        }
        Edit::RemoveY(sig) => {
            cfg.inputs.retain(|b| b.source != sig);
        }
    }
}

/// Pair X and Y samples by time. For each Y sample, find the X
/// sample whose time is nearest-not-greater (piecewise-constant hold
/// of X). Result is a `Vec<[x, y]>` suitable for `PlotPoints::new`.
///
/// Linear scan, O(n + m). Assumes both inputs are time-sorted, which
/// the registry guarantees (samples are appended in order).
fn pair_by_time(xs: &[ScalarSample], ys: impl IntoIterator<Item = ScalarSample>) -> Vec<[f64; 2]> {
    let mut out = Vec::new();
    let mut i = 0;
    for y in ys {
        while i + 1 < xs.len() && xs[i + 1].time <= y.time {
            i += 1;
        }
        // Don't emit until X actually has a sample at-or-before this
        // Y — otherwise the first Y samples would pair with stale X.
        if i < xs.len() && xs[i].time <= y.time {
            out.push([xs[i].value, y.value]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(time: f64, value: f64) -> ScalarSample {
        ScalarSample { time, value }
    }

    #[test]
    fn pair_by_time_emits_one_point_per_y_after_first_x() {
        let xs = vec![s(0.0, 10.0), s(1.0, 20.0), s(2.0, 30.0)];
        let ys = vec![s(0.0, 100.0), s(0.5, 110.0), s(1.0, 120.0), s(1.5, 130.0), s(2.0, 140.0)];
        let got = pair_by_time(&xs, ys);
        // piecewise-constant X: y@0 pairs with x@0=10; y@0.5 still with x@0=10;
        // y@1 with x@1=20; y@1.5 still with x@1=20; y@2 with x@2=30.
        assert_eq!(got, vec![[10.0, 100.0], [10.0, 110.0], [20.0, 120.0], [20.0, 130.0], [30.0, 140.0]]);
    }

    #[test]
    fn pair_by_time_skips_ys_before_first_x() {
        let xs = vec![s(1.0, 10.0)];
        let ys = vec![s(0.0, 100.0), s(0.5, 110.0), s(1.0, 120.0), s(1.5, 130.0)];
        let got = pair_by_time(&xs, ys);
        // Two leading Y samples have no X yet — silently dropped.
        assert_eq!(got, vec![[10.0, 120.0], [10.0, 130.0]]);
    }

    #[test]
    fn pair_by_time_empty_inputs() {
        assert!(pair_by_time(&[], vec![].into_iter()).is_empty());
        assert!(pair_by_time(&[s(0.0, 1.0)], vec![].into_iter()).is_empty());
        assert!(pair_by_time(&[], vec![s(0.0, 1.0)]).is_empty());
    }

    #[test]
    fn line_plot_style_round_trips_through_config_json() {
        use crate::viz::{VisualizationConfig, VizId};
        let mut cfg = VisualizationConfig {
            id: VizId(42),
            title: "t".into(),
            kind: LINE_PLOT_KIND.clone(),
            view: crate::view::ViewTarget::Panel2D,
            inputs: vec![],
            style: serde_json::Value::Null,
        };
        let style = LinePlotStyle {
            x_signal: Some(SignalRef::new(bevy::prelude::Entity::PLACEHOLDER, "phi")),
            x_label: Some("phi [rad]".into()),
            y_label: Some("w [rad/s]".into()),
        };
        style.save(&mut cfg);
        let roundtrip = LinePlotStyle::load(&cfg);
        assert_eq!(roundtrip.x_signal.map(|s| s.path), Some("phi".into()));
        assert_eq!(roundtrip.x_label.as_deref(), Some("phi [rad]"));
        assert_eq!(roundtrip.y_label.as_deref(), Some("w [rad/s]"));
    }
}
