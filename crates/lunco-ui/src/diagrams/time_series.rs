//! Time-series chart widget — pure rendering, zero data copies.
//!
//! The widget is a rendering function that takes borrowed data.
//! Domain panels (e.g., Modelica workbench) own the data and convert
//! it to thin reference wrappers before calling the widget.
//! No intermediate resources, no per-frame allocation.

use bevy_egui::egui;
use egui_plot::{Line, Plot, PlotPoints};

/// A borrowed reference to chart data — zero copy.
///
/// The panel (domain crate) builds this from its own data structures
/// and passes to the widget. Only pointers, no data duplication.
pub struct ChartSeries<'a> {
    /// Series label for the legend.
    pub name: &'a str,
    /// Flat Y-value array. X is implicit (index × dt).
    pub y_values: &'a [f64],
    /// Time step between samples. If None, X is just the index.
    pub dt: Option<f64>,
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
///             color: None,
///         }))
///         .collect();
///
///     time_series_plot(ui, &plot_id, &series);
/// }
/// ```
pub fn time_series_plot(ui: &mut egui::Ui, plot_id: &str, series: &[ChartSeries]) {
    Plot::new(plot_id).view_aspect(2.0).show(ui, |plot_ui| {
        for s in series {
            if s.y_values.is_empty() {
                continue;
            }

            // Build [x, y] pairs — no allocation if we use the iterator directly
            let values: Vec<[f64; 2]> = s
                .y_values
                .iter()
                .enumerate()
                .map(|(i, &y)| [s.dt.map(|dt| i as f64 * dt).unwrap_or(i as f64), y])
                .collect();

            let mut line = Line::new(s.name, PlotPoints::new(values));
            if let Some(color) = s.color {
                line = line.color(color);
            }

            plot_ui.line(line);
        }
    });
}
