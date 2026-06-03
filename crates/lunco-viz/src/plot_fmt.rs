//! Shared `egui_plot` formatting helpers used by every plot surface
//! (the live `LinePlot` here in lunco-viz and the experiments overlay
//! in lunco-modelica). Keeping them in one place means hover-readout
//! and log-scale behave identically across both.

use egui_plot::PlotPoint;

/// Hover-tooltip text for a plot point.
///
/// `name` is the series name egui_plot resolved for the nearest line
/// (empty string when the cursor isn't near a named line). `log_y` is
/// `true` when the Y values were log10-transformed for display, in
/// which case we de-log the value back to its real magnitude so the
/// readout shows what the user actually plotted.
pub fn hover_label(name: &str, point: &PlotPoint, log_y: bool) -> String {
    let y = if log_y { 10f64.powf(point.y) } else { point.y };
    if name.is_empty() {
        // Cursor isn't near a named line — just the coordinates.
        format!("t = {:.4}\n{:.5}", point.x, y)
    } else {
        // Label the value with the signal's own name, not a bare "y".
        format!("t = {:.4}\n{name} = {:.5}", point.x, y)
    }
}

/// Transform a series' points for a log10 Y axis: `y → log10(y)`, with
/// X untouched. Points with `y ≤ 0` are dropped (log undefined there)
/// rather than clamped, so a curve that dips non-positive simply has a
/// gap instead of a misleading floor.
pub fn log_y_points(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    points
        .iter()
        .filter(|p| p[1] > 0.0)
        .map(|p| [p[0], p[1].log10()])
        .collect()
}

/// Y-axis tick label for a log10-transformed axis. The grid mark sits
/// at `log10(value)`, so we raise it back to the real value for the
/// label — scientific notation for very large/small magnitudes,
/// plain decimal otherwise.
pub fn log_y_tick(mark_value: f64) -> String {
    let real = 10f64.powf(mark_value);
    if real != 0.0 && (real >= 1000.0 || real.abs() < 0.001) {
        format!("{real:.0e}")
    } else {
        format!("{real:.4}")
    }
}
