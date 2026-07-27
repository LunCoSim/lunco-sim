//! Time-series chart widget — pure rendering, no per-sample copies.
//!
//! The widget is a rendering function that takes borrowed data.
//! Domain panels (e.g., Modelica workbench) own the data and convert
//! it to thin reference wrappers before calling the widget.
//!
//! The Y slices are handed to `egui_plot` through a generator-backed
//! `PlotPoints` (`from_explicit_callback`), so this module never
//! materialises a `[x, y]` pair per sample — egui_plot samples the
//! closure while tessellating. What remains per frame is O(series)
//! bookkeeping (a boxed closure and axis-label strings), not
//! O(samples) data duplication.

use bevy_egui::egui;
use egui_plot::{Line, Plot, PlotPoints};

/// A borrowed reference to chart data — zero copy.
///
/// The panel (domain crate) builds this from its own data structures
/// and passes to the widget. Only pointers, no data duplication.
pub struct ChartSeries<'a> {
    /// Series label for the legend.
    pub name: &'a str,
    /// Flat Y-value array. X is `t0 + index × dt`.
    pub y_values: &'a [f64],
    /// Time step between samples. If None, X is just the index
    /// (offset by `t0`).
    pub dt: Option<f64>,
    /// Sim time of `y_values[0]`, in seconds. For a ring buffer this
    /// is the time of the *oldest retained* sample, so the X axis
    /// reads in sim time instead of restarting at 0 whenever the ring
    /// wraps. `0.0` when the caller has no time base.
    pub t0: f64,
    /// Physical unit of the Y values (e.g. `"N"`, `"m/s"`), used for
    /// the Y-axis label when every plotted series agrees on it.
    pub unit: Option<&'a str>,
    /// Line color. If None, uses default theme color.
    pub color: Option<egui::Color32>,
}

/// Render a time-series plot. Pure function — no ECS access, no state.
///
/// # Arguments
/// * `ui` — egui UI context
/// * `plot_id` — unique ID for pan/zoom state (egui tracks this)
/// * `series` — borrowed chart data (no copies)
///
/// # Usage from a domain panel
/// ```ignore
/// fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
///     let channels = world.resource::<ModelicaChannels>();
///     let plotted = world.resource::<PlottedVariables>();
///
///     let series: Vec<ChartSeries> = plotted.names.iter()
///         .filter_map(|name| channels.get(name).map(|ch| ChartSeries {
///             name,
///             y_values: ch.history.as_slice(),
///             dt: Some(ch.dt),
///             t0: ch.start_time,
///             unit: ch.unit.as_deref(),
///             color: None,
///         }))
///         .collect();
///
///     time_series_plot(ui, &plot_id, &series);
/// }
/// ```
pub fn time_series_plot(ui: &mut egui::Ui, plot_id: &str, series: &[ChartSeries]) {
    let mut plot = Plot::new(plot_id).view_aspect(2.0);
    // X is sim time whenever any series carries a dt; otherwise it's
    // a bare sample index and labelling it "time (s)" would lie.
    if series.iter().any(|s| s.dt.is_some()) {
        plot = plot.x_axis_label("time (s)");
    } else {
        plot = plot.x_axis_label("sample");
    }
    // Y label: single series → "name (unit)"; several series that all
    // agree on a unit → just the unit; mixed units → no label (the
    // legend carries the names, and a shared label would mislabel at
    // least one line).
    let non_empty: Vec<&ChartSeries> = series.iter().filter(|s| !s.y_values.is_empty()).collect();
    match non_empty.as_slice() {
        [one] => {
            plot = plot.y_axis_label(match one.unit {
                Some(u) if !u.is_empty() => format!("{} ({u})", one.name),
                _ => one.name.to_string(),
            });
        }
        many if !many.is_empty() => {
            let first_unit = many[0].unit.filter(|u| !u.is_empty());
            if let Some(u) = first_unit {
                if many.iter().all(|s| s.unit == Some(u)) {
                    plot = plot.y_axis_label(u.to_string());
                }
            }
        }
        _ => {}
    }

    plot.show(ui, |plot_ui| {
        for s in series {
            let n = s.y_values.len();
            if n == 0 {
                continue;
            }
            let dt = s.dt.unwrap_or(1.0);
            let x0 = s.t0;
            let points = if n == 1 || !(dt > 0.0) {
                // A single sample (or degenerate dt) can't span an X
                // range — the generator's spacing would divide by
                // zero. One owned point is fine.
                PlotPoints::from(vec![[x0, s.y_values[0]]])
            } else {
                // Generator-backed: egui_plot asks the closure for Y
                // at each of `n` evenly spaced X positions, which land
                // exactly on `x0 + i·dt` — no per-sample copy here.
                let y = s.y_values;
                let last = n - 1;
                PlotPoints::from_explicit_callback(
                    move |x| {
                        let i = ((x - x0) / dt).round();
                        let i = (i.max(0.0) as usize).min(last);
                        y[i]
                    },
                    x0..=(x0 + last as f64 * dt),
                    n,
                )
            };

            let mut line = Line::new(s.name, points);
            if let Some(color) = s.color {
                line = line.color(color);
            }

            plot_ui.line(line);
        }
    });
}
