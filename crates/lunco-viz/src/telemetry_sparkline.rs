//! Generic retained-telemetry sparkline.
//!
//! This widget deliberately accepts a [`SignalRef`] instead of a diagnostic
//! name. Engine health, vehicle state, Modelica outputs, and authored channels
//! therefore use the same retained-history path. A status bar may choose a
//! default channel, but it does not get a private frame-time plotter.

use std::sync::Arc;

use egui::{Color32, Id, Response, Sense, Stroke, Ui, Vec2};
use lunco_theme::Theme;

use crate::plot_fmt::decimate_min_max;
use crate::signal::{ScalarHistory, SignalRef, SignalRegistry};

/// Optional reference line and sizing policy for a telemetry sparkline.
#[derive(Clone, Copy, Debug)]
pub struct TelemetrySparklineOptions {
    pub id: Id,
    pub width: f32,
    pub height: f32,
    pub reference_y: Option<f64>,
    pub line_color: Option<Color32>,
}

impl Default for TelemetrySparklineOptions {
    fn default() -> Self {
        Self {
            id: Id::new("telemetry_sparkline"),
            width: 96.0,
            height: 18.0,
            reference_y: None,
            line_color: None,
        }
    }
}

/// Retained statistics for a scalar channel's current history window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TelemetrySparklineStats {
    pub min: f64,
    pub max: f64,
    pub p99: f64,
    pub latest: f64,
}

/// Calculate display statistics without changing the retained signal history.
pub fn telemetry_history_stats(history: &ScalarHistory) -> Option<TelemetrySparklineStats> {
    let mut values = Vec::with_capacity(history.len());
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut latest = None;
    for sample in history.iter() {
        min = min.min(sample.value);
        max = max.max(sample.value);
        latest = Some(sample.value);
        values.push(sample.value);
    }
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let p99 = values[((values.len() as f64 * 0.99) as usize).min(values.len() - 1)];
    Some(TelemetrySparklineStats {
        min,
        max,
        p99,
        latest: latest.expect("non-empty history has a latest sample"),
    })
}

/// Read the last screen-ready statistics for a widget without touching the
/// retained history. The status bar uses this for its optional p99 label; the
/// first frame intentionally omits that optional detail until the widget has
/// built its cache.
pub fn cached_telemetry_sparkline_stats(
    ctx: &egui::Context,
    id: Id,
    signal: &SignalRef,
) -> Option<TelemetrySparklineStats> {
    ctx.data(|data| {
        data.get_temp::<Arc<CachedSparkline>>(id.with("history"))
            .filter(|cached| cached.signal == *signal)
            .and_then(|cached| cached.stats)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HistoryFingerprint {
    len: usize,
    first_time: u64,
    last_time: u64,
    last_value: u64,
}

fn history_fingerprint(history: &ScalarHistory) -> HistoryFingerprint {
    HistoryFingerprint {
        len: history.len(),
        first_time: history
            .samples
            .front()
            .map_or(0, |sample| sample.time.to_bits()),
        last_time: history
            .samples
            .back()
            .map_or(0, |sample| sample.time.to_bits()),
        last_value: history
            .samples
            .back()
            .map_or(0, |sample| sample.value.to_bits()),
    }
}

#[derive(Clone)]
struct CachedSparkline {
    signal: SignalRef,
    fingerprint: HistoryFingerprint,
    width: u32,
    points: Vec<[f64; 2]>,
    stats: Option<TelemetrySparklineStats>,
}

/// Paint a compact line for one retained scalar telemetry channel.
///
/// The history is copied and decimated only when the channel's retained
/// fingerprint or the pixel width changes. Stable UI frames reuse the cached
/// screen-ready points; the `SignalRegistry` remains the sole sample owner.
pub fn render_telemetry_sparkline(
    ui: &mut Ui,
    registry: &SignalRegistry,
    signal: &SignalRef,
    theme: &Theme,
    options: TelemetrySparklineOptions,
) -> (Response, Option<TelemetrySparklineStats>) {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(options.width.max(1.0), options.height.max(1.0)),
        Sense::hover(),
    );
    let Some(history) = registry.scalar_history(signal) else {
        return (response, None);
    };
    let fingerprint = history_fingerprint(history);
    let width = rect.width().round().max(1.0) as u32;
    let cache_id = options.id.with("history");
    let cached = ui.ctx().data(|data| {
        data.get_temp::<Arc<CachedSparkline>>(cache_id)
            .filter(|cached| {
                cached.signal == *signal
                    && cached.fingerprint == fingerprint
                    && cached.width == width
            })
    });
    let cached = cached.unwrap_or_else(|| {
        let raw: Vec<[f64; 2]> = history
            .iter()
            .map(|sample| [sample.time, sample.value])
            .collect();
        let points = decimate_min_max(&raw, rect.width()).unwrap_or(raw);
        let cached = Arc::new(CachedSparkline {
            signal: signal.clone(),
            fingerprint,
            width,
            points,
            stats: telemetry_history_stats(history),
        });
        ui.ctx()
            .data_mut(|data| data.insert_temp(cache_id, cached.clone()));
        cached
    });

    let Some(stats) = cached.stats else {
        return (response, None);
    };
    let mut low = stats.min;
    let mut high = stats.max;
    if let Some(reference) = options.reference_y {
        low = low.min(reference);
        high = high.max(reference);
    }
    let span = (high - low).abs();
    let padding = if span > 0.0 {
        span * 0.08
    } else {
        high.abs().max(1.0) * 0.08
    };
    low -= padding;
    high += padding;
    let y_span = (high - low).max(f64::EPSILON);
    let x0 = cached.points.first().map_or(0.0, |point| point[0]);
    let x1 = cached.points.last().map_or(1.0, |point| point[0]);
    let x_span = x1 - x0;
    let line_color = options
        .line_color
        .unwrap_or_else(|| crate::signal::color_for_signal(theme, &signal.path));
    let painter = ui.painter_at(rect);
    let to_screen = |index: usize, point: [f64; 2]| {
        let x = if x_span > 0.0 {
            (point[0] - x0) / x_span
        } else if cached.points.len() > 1 {
            index as f64 / (cached.points.len() - 1) as f64
        } else {
            0.5
        };
        let y = (point[1] - low) / y_span;
        egui::pos2(
            rect.left() + rect.width() * x as f32,
            rect.bottom() - rect.height() * y as f32,
        )
    };
    let mut previous = None;
    for (index, point) in cached.points.iter().copied().enumerate() {
        let current = to_screen(index, point);
        if let Some(previous) = previous {
            painter.line_segment([previous, current], Stroke::new(1.0, line_color));
        }
        previous = Some(current);
    }
    if let Some(reference) = options.reference_y {
        let y = rect.bottom() - rect.height() * ((reference - low) / y_span) as f32;
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            Stroke::new(0.5, theme.tokens.text_subdued.gamma_multiply(0.7)),
        );
    }
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(0.5, theme.tokens.text_subdued.gamma_multiply(0.55)),
        egui::StrokeKind::Inside,
    );
    (response, Some(stats))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ScalarSample;

    #[test]
    fn history_stats_are_generic_and_keep_native_values() {
        let mut history = ScalarHistory::new(8);
        for value in [1.0, 2.0, 3.0, 100.0] {
            history.push(ScalarSample { time: value, value });
        }

        let stats = telemetry_history_stats(&history).expect("history statistics");
        assert_eq!(stats.min, 1.0);
        assert_eq!(stats.max, 100.0);
        assert_eq!(stats.latest, 100.0);
        assert_eq!(stats.p99, 100.0);
    }
}
