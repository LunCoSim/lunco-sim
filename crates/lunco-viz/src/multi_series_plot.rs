//! Reusable multi-series plot widget for completed and live trajectories.
//!
//! Domain panels prepare shared point buffers and presentation labels; this
//! module owns only egui plot policy: legends, line styles, log-Y rendering,
//! hover text, fit requests, overlays, and the optional scrub cursor.

use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use egui;
use egui_plot::{
    ClosestElem, Legend, Line, LineStyle, Plot, PlotBounds, PlotGeometry, PlotItem, PlotPoint,
    PlotPoints, PlotTransform, PlotUi, VLine,
};
use std::sync::{Arc, Mutex};

struct CachedPlotSeries {
    points: Arc<Vec<PlotPoint>>,
    bounds: PlotBounds,
}

#[derive(Default)]
struct MultiSeriesPointsCache {
    requested: Option<(egui::Id, Arc<Vec<[f64; 2]>>, bool, u32)>,
    displayed: Option<(
        egui::Id,
        Arc<Vec<[f64; 2]>>,
        bool,
        u32,
        Arc<CachedPlotSeries>,
    )>,
    build: Option<
        Task<(
            egui::Id,
            Arc<Vec<[f64; 2]>>,
            bool,
            u32,
            Arc<CachedPlotSeries>,
        )>,
    >,
}

#[derive(Default)]
struct MultiSeriesPlotCache {
    items: Vec<MultiSeriesPointsCache>,
}

/// Retain the line's data bounds with its immutable plot points. egui_plot
/// otherwise walks every point again on every frame while auto-fitting axes.
struct CachedBoundsLine<'a> {
    line: Line<'a>,
    bounds: PlotBounds,
    monotonic_x: bool,
}

impl egui_plot::PlotItem for CachedBoundsLine<'_> {
    fn shapes(
        &self,
        ui: &egui::Ui,
        transform: &egui_plot::PlotTransform,
        shapes: &mut Vec<egui::Shape>,
    ) {
        self.line.shapes(ui, transform, shapes);
    }

    fn initialize(&mut self, x_range: std::ops::RangeInclusive<f64>) {
        self.line.initialize(x_range);
    }

    fn color(&self) -> egui::Color32 {
        PlotItem::color(&self.line)
    }

    fn geometry(&self) -> egui_plot::PlotGeometry<'_> {
        self.line.geometry()
    }

    fn find_closest(&self, pointer: egui::Pos2, transform: &PlotTransform) -> Option<ClosestElem> {
        if self.monotonic_x {
            if let PlotGeometry::Points(points) = self.line.geometry() {
                return closest_monotonic_line_point(points, pointer, transform);
            }
        }
        self.line.find_closest(pointer, transform)
    }

    fn bounds(&self) -> PlotBounds {
        self.bounds
    }

    fn base(&self) -> &egui_plot::PlotItemBase {
        self.line.base()
    }

    fn base_mut(&mut self) -> &mut egui_plot::PlotItemBase {
        self.line.base_mut()
    }
}

pub(crate) fn add_cached_line<'a>(
    plot_ui: &mut PlotUi<'a>,
    line: Line<'a>,
    bounds: PlotBounds,
    monotonic_x: bool,
) {
    plot_ui.add(CachedBoundsLine {
        line,
        bounds,
        monotonic_x,
    });
}

/// Find the nearest line segment while pruning segments whose X interval is
/// already farther from the pointer than the best candidate. Time-series X
/// values are ordered, so the search is logarithmic plus the nearby segments
/// that could beat the current distance. Non-monotonic phase-space lines use
/// egui_plot's general search.
fn closest_monotonic_line_point(
    points: &[PlotPoint],
    pointer: egui::Pos2,
    transform: &PlotTransform,
) -> Option<ClosestElem> {
    match points.len() {
        0 => return None,
        1 => {
            return Some(ClosestElem {
                index: 0,
                dist_sq: pointer.distance_sq(transform.position_from_point(&points[0])),
            });
        }
        _ => {}
    }

    let pointer_x = transform.value_from_position(pointer).x;
    let insertion = points.partition_point(|sample| sample.x < pointer_x);
    let last_segment = points.len() - 2;
    let center = insertion.saturating_sub(1).min(last_segment);
    let mut best_segment = center;
    let mut best = closest_line_segment(points, center, pointer, transform);

    let mut left = center;
    while left > 0 {
        let candidate = left - 1;
        let nearest_x = transform.position_from_point_x(points[candidate + 1].x);
        let min_dist_sq = (pointer.x - nearest_x).powi(2);
        if min_dist_sq > best.dist_sq {
            break;
        }
        let closest = closest_line_segment(points, candidate, pointer, transform);
        if closest.dist_sq.total_cmp(&best.dist_sq).is_lt()
            || (closest.dist_sq == best.dist_sq && candidate < best_segment)
        {
            best = closest;
            best_segment = candidate;
        }
        left = candidate;
    }

    let mut right = center + 1;
    while right <= last_segment {
        let nearest_x = transform.position_from_point_x(points[right].x);
        let min_dist_sq = (pointer.x - nearest_x).powi(2);
        if min_dist_sq > best.dist_sq {
            break;
        }
        let closest = closest_line_segment(points, right, pointer, transform);
        if closest.dist_sq.total_cmp(&best.dist_sq).is_lt()
            || (closest.dist_sq == best.dist_sq && right < best_segment)
        {
            best = closest;
            best_segment = right;
        }
        right += 1;
    }

    Some(best)
}

fn closest_line_segment(
    points: &[PlotPoint],
    segment: usize,
    pointer: egui::Pos2,
    transform: &PlotTransform,
) -> ClosestElem {
    let first = transform.position_from_point(&points[segment]);
    let second = transform.position_from_point(&points[segment + 1]);
    let delta = second - first;
    let delta_len_sq = delta.length_sq();
    let closest = if delta_len_sq == 0.0 {
        first
    } else {
        let along = (pointer - first).dot(delta) / delta_len_sq;
        first + along.clamp(0.0, 1.0) * delta
    };
    let first_index = if pointer.distance_sq(first) <= pointer.distance_sq(second) {
        segment
    } else {
        segment + 1
    };
    ClosestElem {
        index: first_index,
        dist_sq: pointer.distance_sq(closest),
    }
}

fn cached_plot_points(
    ctx: &egui::Context,
    cache: &mut MultiSeriesPointsCache,
    identity: egui::Id,
    source: &Arc<Vec<[f64; 2]>>,
    log_y: bool,
    pixel_width: u32,
) -> Option<Arc<CachedPlotSeries>> {
    let completed = cache
        .build
        .as_mut()
        .and_then(|task| future::block_on(future::poll_once(task)));
    if let Some((built_identity, built_source, built_log_y, built_pixel_width, points)) = completed
    {
        cache.build = None;
        if cache.requested.as_ref().is_some_and(
            |(requested_identity, requested, requested_log_y, requested_pixel_width)| {
                *requested_identity == built_identity
                    && Arc::ptr_eq(requested, &built_source)
                    && *requested_log_y == built_log_y
                    && *requested_pixel_width == built_pixel_width
            },
        ) {
            cache.displayed = Some((
                built_identity,
                built_source,
                built_log_y,
                built_pixel_width,
                points,
            ));
        }
    }

    if !cache.requested.as_ref().is_some_and(
        |(requested_identity, requested, requested_log_y, requested_pixel_width)| {
            *requested_identity == identity
                && Arc::ptr_eq(requested, source)
                && *requested_log_y == log_y
                && *requested_pixel_width == pixel_width
        },
    ) {
        cache.requested = Some((identity, Arc::clone(source), log_y, pixel_width));
    }

    let displayed_matches = cache.displayed.as_ref().is_some_and(
        |(displayed_identity, displayed, displayed_log_y, displayed_pixel_width, _)| {
            *displayed_identity == identity
                && Arc::ptr_eq(displayed, source)
                && *displayed_log_y == log_y
                && *displayed_pixel_width == pixel_width
        },
    );
    if !displayed_matches && cache.build.is_none() {
        let build_identity = identity;
        let source = Arc::clone(source);
        let build_pixel_width = pixel_width;
        cache.build = Some(AsyncComputeTaskPool::get().spawn(async move {
            let mut samples: Vec<[f64; 2]> = source
                .iter()
                .filter_map(|[x, y]| {
                    let y = if log_y {
                        (*y > 0.0).then(|| y.log10())?
                    } else {
                        *y
                    };
                    Some([*x, y])
                })
                .collect();
            let mut bounds = PlotBounds::NOTHING;
            for sample in &samples {
                bounds.extend_with(&PlotPoint::from(*sample));
            }
            if let Some(decimated) =
                crate::plot_fmt::decimate_min_max(&samples, build_pixel_width as f32 * 0.5)
            {
                samples = decimated;
            }
            let points: Vec<PlotPoint> = samples.into_iter().map(PlotPoint::from).collect();
            (
                build_identity,
                source,
                log_y,
                build_pixel_width,
                Arc::new(CachedPlotSeries {
                    points: Arc::new(points),
                    bounds,
                }),
            )
        }));
    }
    if cache.build.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }

    cache
        .displayed
        .as_ref()
        .filter(|(displayed_identity, _, displayed_log_y, _, _)| {
            *displayed_identity == identity && *displayed_log_y == log_y
        })
        .map(|(_, _, _, _, points)| Arc::clone(points))
}

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
    /// Stable identity across history updates and display-width rebuilds.
    pub cache_key: egui::Id,
    /// Legend and hover label.
    pub label: String,
    /// Curve colour.
    pub color: egui::Color32,
    /// Shared time-value samples.
    pub points: std::sync::Arc<Vec<[f64; 2]>>,
    /// Whether every plotted Y sample is strictly positive.
    pub all_positive: bool,
    /// Stroke used to distinguish related curves.
    pub style: MultiSeriesStyle,
}

/// An additional trajectory overlaid on the completed-run curves.
pub struct MultiSeriesOverlay {
    /// Stable identity across history updates and display-width rebuilds.
    pub cache_key: egui::Id,
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
    type SharedPlotCache = Arc<Mutex<MultiSeriesPlotCache>>;
    let cache_id = options.id.with("multi_series_plot_points");
    let cache: SharedPlotCache = ui.ctx().data_mut(|data| {
        if let Some(existing) = data.get_temp::<SharedPlotCache>(cache_id) {
            existing
        } else {
            let fresh = SharedPlotCache::default();
            data.insert_temp(cache_id, Arc::clone(&fresh));
            fresh
        }
    });
    let mut plot_cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let item_count = series.len() + overlays.len();
    plot_cache.items.resize_with(item_count, Default::default);
    plot_cache.items.truncate(item_count);

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

    let egui_ctx = ui.ctx().clone();
    let pixel_width = ui.available_width().max(1.0) as u32;
    let plotted_series: Vec<_> = series
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let points = cached_plot_points(
                &egui_ctx,
                &mut plot_cache.items[index],
                line.cache_key,
                &line.points,
                options.log_y,
                pixel_width,
            )?;
            let style = match line.style {
                MultiSeriesStyle::Solid => LineStyle::Solid,
                MultiSeriesStyle::Dashed => LineStyle::dashed_dense(),
                MultiSeriesStyle::Dotted => LineStyle::dotted_dense(),
                MultiSeriesStyle::DashDot => LineStyle::dashed_loose(),
            };
            Some((line.label.clone(), line.color, style, points))
        })
        .collect();
    let plotted_overlays: Vec<_> = overlays
        .iter()
        .enumerate()
        .filter_map(|(index, overlay)| {
            let points = cached_plot_points(
                &egui_ctx,
                &mut plot_cache.items[series.len() + index],
                overlay.cache_key,
                &overlay.points,
                options.log_y,
                pixel_width,
            )?;
            Some((overlay.label.clone(), overlay.color, points))
        })
        .collect();
    drop(plot_cache);
    let mut clicked_x = None;
    plot.show(ui, |plot_ui| {
        for (label, color, style, points) in &plotted_series {
            add_cached_line(
                plot_ui,
                Line::new(label.clone(), PlotPoints::from(points.points.as_slice()))
                    .color(*color)
                    .style(*style),
                points.bounds,
                true,
            );
        }
        for (label, color, points) in &plotted_overlays {
            add_cached_line(
                plot_ui,
                Line::new(label.clone(), PlotPoints::from(points.points.as_slice())).color(*color),
                points.bounds,
                true,
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
