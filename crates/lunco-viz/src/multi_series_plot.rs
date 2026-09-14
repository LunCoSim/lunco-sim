//! Reusable multi-series plot widget for completed and live trajectories.
//!
//! Domain panels prepare shared point buffers and presentation labels; this
//! module owns only egui plot policy: legends, line styles, log-Y rendering,
//! hover text, fit requests, overlays, and the optional scrub cursor.

use bevy_egui::egui;
use egui_plot::{Legend, Line, LineStyle, Plot, PlotPoints, VLine};

/// Stroke style for a multi-series curve.
#[derive(Debug, Clone, Copy, Default)]
pub enum MultiSeriesStyle {
    /// Continuous stroke.
    #[default]
    Solid,
    /// Dense dashed stroke.
    Dashed,
    /// Dense dotted stroke.
    Dotted,
    /// Loose dashed stroke.
    DashDot,
}

/// A labelled trajectory prepared by a domain adapter.
pub struct MultiSeriesLine {
    /// Legend and hover label.
    pub label: String,
    /// Curve colour.
    pub color: egui::Color32,
    /// Shared time-value samples.
    pub points: std::sync::Arc<Vec<[f64; 2]>>,
    /// Stroke used to distinguish related curves.
    pub style: MultiSeriesStyle,
}

/// An additional trajectory overlaid on the completed-run curves.
pub struct MultiSeriesOverlay {
    /// Legend and hover label.
    pub label: String,
    /// Curve colour.
    pub color: egui::Color32,
    /// Shared time-value samples.
    pub points: std::sync::Arc<Vec<[f64; 2]>>,
}

/// Rendering options for [`render_multi_series_plot`].
pub struct MultiSeriesPlotOptions {
    /// egui identity; each visible plot must provide a distinct id.
    pub id: egui::Id,
    /// Transform positive Y values to log10 before rendering.
    pub log_y: bool,
    /// Reset egui's remembered bounds before painting.
    pub reset_bounds: bool,
    /// Optional shared unit label for the Y axis.
    pub y_axis_label: Option<String>,
    /// Optional X coordinate at which to paint a scrub cursor.
    pub scrub_time: Option<f64>,
    /// Cursor colour.
    pub scrub_color: egui::Color32,
}

impl Default for MultiSeriesPlotOptions {
    fn default() -> Self {
        Self {
            id: egui::Id::new("lunco_multi_series_plot"),
            log_y: false,
            reset_bounds: false,
            y_axis_label: None,
            scrub_time: None,
            scrub_color: egui::Color32::WHITE,
        }
    }
}

/// Render a multi-series plot and return the X coordinate clicked this frame.
///
/// The caller owns the resulting scrub state. The function performs no ECS
/// access and accepts shared point buffers so constructing a frame's input is
/// pointer-only after the domain cache has been built.
pub fn render_multi_series_plot(
    ui: &mut egui::Ui,
    series: &[MultiSeriesLine],
    overlays: &[MultiSeriesOverlay],
    options: &MultiSeriesPlotOptions,
) -> Option<f64> {
    let mut plot = Plot::new(options.id)
        .legend(Legend::default())
        .allow_drag(false)
        .label_formatter(|pos| {
            let (name, point) = match pos {
                egui_plot::HoverPosition::NearDataPoint {
                    plot_name,
                    position,
                    ..
                } => (*plot_name, position),
                egui_plot::HoverPosition::Elsewhere { position } => ("", position),
            };
            Some(crate::plot_fmt::hover_label(name, point, options.log_y))
        });
    if options.reset_bounds {
        plot = plot.reset();
    }
    if options.log_y {
        plot = plot.y_axis_formatter(|mark, _range| crate::plot_fmt::log_y_tick(mark.value));
    }
    if let Some(label) = options.y_axis_label.as_deref() {
        plot = plot.y_axis_label(label.to_owned());
    }

    let mut clicked_x = None;
    plot.show(ui, |plot_ui| {
        for line in series {
            let points = if options.log_y {
                crate::plot_fmt::log_y_points(line.points.as_slice())
            } else {
                line.points.as_ref().clone()
            };
            let style = match line.style {
                MultiSeriesStyle::Solid => LineStyle::Solid,
                MultiSeriesStyle::Dashed => LineStyle::dashed_dense(),
                MultiSeriesStyle::Dotted => LineStyle::dotted_dense(),
                MultiSeriesStyle::DashDot => LineStyle::dashed_loose(),
            };
            plot_ui.line(
                Line::new(line.label.clone(), PlotPoints::from(points))
                    .color(line.color)
                    .style(style),
            );
        }
        for overlay in overlays {
            let points = if options.log_y {
                crate::plot_fmt::log_y_points(overlay.points.as_slice())
            } else {
                overlay.points.as_ref().clone()
            };
            plot_ui.line(
                Line::new(overlay.label.clone(), PlotPoints::from(points)).color(overlay.color),
            );
        }
        if let Some(time) = options.scrub_time {
            plot_ui.vline(
                VLine::new("scrub", time)
                    .color(options.scrub_color)
                    .width(1.5),
            );
        }
        if plot_ui.response().clicked() {
            clicked_x = plot_ui.pointer_coordinate().map(|point| point.x);
        }
    });
    clicked_x
}
