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

/// Min-max decimation of a time-sorted series to a pixel budget.
///
/// Each pixel column keeps the sample with the smallest and the sample
/// with the largest Y that fall inside it (emitted in original sample
/// order, so X stays monotone) — a single-sample spike therefore
/// survives decimation instead of being averaged away. Returns `None`
/// when the series is already small enough that decimating would not
/// meaningfully shrink it (caller plots the original), or when the X
/// span is degenerate (all samples at one X — nothing to bucket by).
///
/// Only valid for X-monotone series (classic time-series). Phase-space
/// trajectories revisit X and must not be decimated this way.
pub fn decimate_min_max(points: &[[f64; 2]], px_width: f32) -> Option<Vec<[f64; 2]>> {
    let cols = (px_width.max(1.0) as usize).max(1);
    // 2 points per column; only worth doing when it at least halves
    // the point count.
    if points.len() <= cols * 4 {
        return None;
    }
    let x0 = points[0][0];
    let span = points[points.len() - 1][0] - x0;
    if !(span > 0.0) || !span.is_finite() {
        return None;
    }
    let mut out = Vec::with_capacity(cols * 2);
    let mut bucket = 0usize;
    // Index of the current bucket's min/max sample (by Y).
    let (mut lo, mut hi): (usize, usize) = (0, 0);
    let flush = |lo: usize, hi: usize, out: &mut Vec<[f64; 2]>| {
        // Emit in sample order so X stays monotone within the column.
        let (a, b) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        out.push(points[a]);
        if b != a {
            out.push(points[b]);
        }
    };
    for (i, p) in points.iter().enumerate() {
        let col = (((p[0] - x0) / span) * cols as f64) as usize;
        let col = col.min(cols - 1);
        if col != bucket {
            flush(lo, hi, &mut out);
            bucket = col;
            lo = i;
            hi = i;
        } else {
            if p[1] < points[lo][1] {
                lo = i;
            }
            if p[1] > points[hi][1] {
                hi = i;
            }
        }
    }
    flush(lo, hi, &mut out);
    Some(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimation_keeps_a_single_sample_spike() {
        // 10k flat samples with one spike in the middle; decimated to
        // 100 px the spike must survive as the max of its column.
        let mut pts: Vec<[f64; 2]> = (0..10_000).map(|i| [i as f64, 0.0]).collect();
        pts[5_000][1] = 42.0;
        let out = decimate_min_max(&pts, 100.0).expect("10k points must decimate at 100 px");
        assert!(out.len() <= 2 * 100 + 2);
        assert!(
            out.iter().any(|p| p[1] == 42.0),
            "min-max buckets must preserve the spike"
        );
        // X must stay monotone so the line doesn't zig-zag backwards.
        assert!(out.windows(2).all(|w| w[0][0] <= w[1][0]));
    }

    #[test]
    fn small_series_pass_through_undedecimated() {
        let pts: Vec<[f64; 2]> = (0..50).map(|i| [i as f64, i as f64]).collect();
        assert!(decimate_min_max(&pts, 100.0).is_none());
    }

    #[test]
    fn degenerate_x_span_is_left_alone() {
        let pts: Vec<[f64; 2]> = (0..5_000).map(|i| [1.0, i as f64]).collect();
        assert!(decimate_min_max(&pts, 10.0).is_none());
    }
}
