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
/// label, then format it compactly — no trailing zeros, and scientific
/// notation only for extreme magnitudes — so a decade axis reads
/// `1, 10, 100, 1000, 1e4` instead of `1.0000, 10.0000, … 1e3`.
pub fn log_y_tick(mark_value: f64) -> String {
    compact_number(10f64.powf(mark_value))
}

/// Format a value with ~3 significant figures, trimming trailing zeros.
/// Magnitudes ≥ 1e4 or < 1e-3 switch to scientific notation (also with a
/// trimmed mantissa, e.g. `1e4`, `2.5e-5`); everything else is a plain
/// decimal, e.g. `1`, `31.6`, `0.001`.
fn compact_number(v: f64) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    if !v.is_finite() {
        return format!("{v}");
    }
    let exp = v.abs().log10().floor() as i32;
    if exp >= 4 || exp < -3 {
        // Scientific: one mantissa decimal, then drop a trailing ".0".
        format!("{v:.1e}").replace(".0e", "e")
    } else {
        // Plain decimal with just enough places for 3 sig-figs, trimmed.
        let decimals = (2 - exp).max(0) as usize;
        let s = format!("{v:.decimals$}");
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    }
}
