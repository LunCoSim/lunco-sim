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
        let build_span = bevy::log::info_span!(
            "multi_series_plot_points_build_worker",
            identity = ?identity,
            input_points = source.len(),
            pixel_width = build_pixel_width,
            log_y,
            output_points = bevy::log::tracing::field::Empty
        );
        cache.build = Some(AsyncComputeTaskPool::get().spawn(async move {
            let _build_span = build_span.enter();
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
            build_span.record("output_points", points.len());
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
    /// Pointer value labels/crosshairs; axis labels remain visible when false.
    pub show_hover_values: bool,
    /// egui identity; each visible plot must provide a distinct id.
    pub id: egui::Id,
    /// Transform positive Y values to log10 before rendering.
    pub log_y: bool,
    /// Reset egui's remembered bounds before painting.
    pub reset_bounds: bool,
    /// Optional shared unit label for the Y axis.
    pub y_axis_label: Option<String>,
    /// Authored X-axis quantity and units.
    pub x_axis_label: Option<String>,
    /// Curve thickness in logical pixels, shared by experiment and live series.
    pub line_width: f32,
    /// Plot-only typography; surrounding panels keep their own theme sizes.
    pub text_size: Option<f32>,
    pub show_legend: bool,
    pub simplified_grid: bool,
    pub compact_layout: bool,
    pub show_latest_value: bool,
    pub caption: Option<String>,
    /// Physical Y values retained in automatic bounds without clipping data.
    pub y_reference_range: Option<[f64; 2]>,
    /// Keep the current X span and advance its right edge with the data.
    pub follow_latest: bool,
    /// Optional span applied when following is enabled or this value changes.
    pub time_window_s: Option<f64>,
    pub time_window_revision: u64,
    /// Optional X coordinate at which to paint a scrub cursor.
    pub scrub_time: Option<f64>,
    /// Cursor colour.
    pub scrub_color: egui::Color32,
}

impl Default for MultiSeriesPlotOptions {
    fn default() -> Self {
        Self {
            id: egui::Id::new("lunco_multi_series_plot"),
            show_hover_values: true,
            log_y: false,
            reset_bounds: false,
            y_axis_label: None,
            x_axis_label: None,
            line_width: 1.5,
            text_size: None,
            show_legend: true,
            simplified_grid: false,
            compact_layout: false,
            show_latest_value: false,
            caption: None,
            y_reference_range: None,
            follow_latest: false,
            time_window_s: None,
            time_window_revision: 0,
            scrub_time: None,
            scrub_color: egui::Color32::WHITE,
        }
    }
}

/// Follow data endpoints without fitting the growing history. A requested
/// window seeds the view once; subsequent user zoom keeps its chosen width.
/// The caller passes only the bounds of series actually shown in this plot.
pub fn follow_latest_x(
    ui: &mut PlotUi,
    ctx: &egui::Context,
    id: egui::Id,
    data: Option<[f64; 2]>,
    enabled: bool,
    requested_window_s: Option<f64>,
    request_revision: u64,
) {
    // Keep a requested initial window pending until there is data to follow.
    // Otherwise an empty first frame consumes the configuration change and
    // the first sample inherits egui's default bounds instead of that window.
    if enabled && data.is_none() {
        return;
    }
    let previous = crate::scene_state::scene_data(ctx, |state| {
        let key = id.with("follow_latest_configuration");
        let previous = state.get_temp::<(bool, Option<f64>, u64)>(key);
        state.insert_temp(key, (enabled, requested_window_s, request_revision));
        previous
    });
    let mut auto = ui.auto_bounds();
    if !enabled {
        if previous.is_some_and(|value| value.0) {
            auto.x = true;
            ui.set_auto_bounds(auto);
        }
        return;
    }
    let Some([first, latest]) = data else { return };
    // Toggling the mode keeps the current view, even if an earlier numeric
    // request is still stored. Only a new requested span seeds fresh bounds.
    let window_changed =
        previous.map(|value| (value.1, value.2)) != Some((requested_window_s, request_revision));
    let width = if window_changed {
        requested_window_s.unwrap_or(ui.plot_bounds().width())
    } else {
        ui.plot_bounds().width()
    };
    let Some([start, end]) = following_bounds(first, latest, width) else {
        return;
    };
    auto.x = false;
    ui.set_auto_bounds(auto);
    ui.set_plot_bounds_x(start..=end);
}

fn following_bounds(first: f64, latest: f64, width: f64) -> Option<[f64; 2]> {
    if !first.is_finite()
        || !latest.is_finite()
        || latest < first
        || !width.is_finite()
        || width <= 0.0
    {
        return None;
    }
    let end = latest.max(first + width);
    (end.is_finite()).then_some([end - width, end])
}

pub struct MultiSeriesPlotResponse {
    pub clicked_x: Option<f64>,
    pub response: egui::Response,
    pub time_span_s: f64,
}

/// Scope typography to the graph. All plot text roles (ticks, axis titles,
/// legend and hover values) grow together without scaling the application UI.
pub(crate) fn with_plot_text<R>(
    ui: &mut egui::Ui,
    size: Option<f32>,
    draw: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.scope(|ui| {
        if let Some(size) = size.filter(|size| size.is_finite() && (8.0..=64.0).contains(size)) {
            for font in ui.style_mut().text_styles.values_mut() {
                font.size = size;
            }
        }
        draw(ui)
    })
    .inner
}

/// Major guides follow a 1/2/5 spacing with about five intervals per axis.
/// The native plot still owns tick placement, bounds and unit mapping.
pub(crate) fn simplify_grid(plot: Plot<'_>) -> Plot<'_> {
    plot.x_grid_spacer(major_grid_marks)
        .y_grid_spacer(major_grid_marks)
        .grid_color(egui::Color32::from_white_alpha(36))
        .grid_fade(0.0)
}

fn major_grid_marks(input: egui_plot::GridInput) -> Vec<egui_plot::GridMark> {
    let (low, high) = input.bounds;
    let raw = (high - low) / 5.0;
    if !low.is_finite() || !high.is_finite() || !raw.is_finite() || raw <= 0.0 {
        return Vec::new();
    }
    let scale = 10_f64.powf(raw.log10().floor());
    if !scale.is_finite() || scale <= 0.0 { return Vec::new(); }
    let fraction = raw / scale;
    let multiple = if fraction <= 1.0 { 1.0 } else if fraction <= 2.0 { 2.0 }
        else if fraction <= 5.0 { 5.0 } else { 10.0 };
    let step = scale * multiple;
    let first = (low / step).ceil() * step;
    (0..=12).map(|i| first + f64::from(i) * step)
        .take_while(|value| value.is_finite() && *value <= high)
        .map(|value| egui_plot::GridMark { value, step_size: step }).collect()
}

/// Compact axis typography uses the same native transform and major grid marks.
/// Hiding outer axis widgets reclaims their margins; values and units remain
/// bound to the plot's actual bounds, including zoom, following and log-Y.
pub(crate) fn render_compact_labels(
    ui: &egui::Ui, transform: &PlotTransform, x: Option<&str>, y: Option<&str>,
    title: Option<&str>, size: Option<f32>, log_y: bool, latest_value: Option<f64>,
) {
    let frame = transform.frame();
    let mut font = egui::TextStyle::Body.resolve(ui.style());
    if let Some(size) = size { font.size = size; }
    let painter = ui.painter().with_clip_rect(*frame);
    let color = ui.visuals().strong_text_color();
    let mut occupied = Vec::new();
    let mut label = |text: &str, pos: egui::Pos2, anchor: egui::Align2, required: bool| {
        let galley = painter.layout_no_wrap(text.to_owned(), font.clone(), color);
        let rect = anchor.anchor_rect(egui::Rect::from_min_size(pos, galley.size()));
        if !required && (!frame.shrink(3.0).contains_rect(rect)
            || occupied.iter().any(|other: &egui::Rect| other.expand(6.0).intersects(rect))) { return; }
        painter.galley(rect.min, galley, color);
        occupied.push(rect);
    };
    if let Some(y) = y {
        label(y, frame.left_top() + egui::vec2(10.0,8.0), egui::Align2::LEFT_TOP, true);
    }
    if let Some(title) = title {
        label(title, frame.center_top() + egui::vec2(0.0,8.0), egui::Align2::CENTER_TOP, true);
    }
    if let Some(value) = latest_value.filter(|value| value.is_finite()) {
        let value = format!("{value:.1} {}",y.unwrap_or_default());
        label(&value,frame.right_top()+egui::vec2(-10.0,8.0),egui::Align2::RIGHT_TOP,true);
    }
    if let Some(x) = x {
        label(x, frame.right_bottom() - egui::vec2(10.0,8.0), egui::Align2::RIGHT_BOTTOM, true);
    }
    let bounds = transform.bounds();
    let marks = |axis: usize| major_grid_marks(egui_plot::GridInput {
        bounds: (bounds.min()[axis],bounds.max()[axis]),base_step_size:0.0,
    });
    let format_tick = |mark: egui_plot::GridMark| {
        let decimals = (-mark.step_size.log10().floor()).clamp(0.0,12.0) as usize;
        egui::emath::format_with_decimals_in_range(mark.value,decimals..=decimals)
    };
    for mark in marks(1) {
        let value = if log_y { crate::plot_fmt::log_y_tick(mark.value) } else { format_tick(mark) };
        let pos = transform.position_from_point(&PlotPoint::new(bounds.min()[0],mark.value));
        label(&value,egui::pos2(frame.left()+10.0,pos.y),egui::Align2::LEFT_CENTER,false);
    }
    for mark in marks(0) {
        let pos = transform.position_from_point(&PlotPoint::new(mark.value,bounds.min()[1]));
        label(&format_tick(mark),egui::pos2(pos.x,frame.bottom()-8.0),egui::Align2::CENTER_BOTTOM,false);
    }
}

pub(crate) fn render_plot_caption(ui: &mut egui::Ui, caption: Option<&str>, size: Option<f32>) {
    if let Some(caption) = caption {
        with_plot_text(ui, size, |ui| {
            ui.label(egui::RichText::new(caption).strong());
            ui.add_space(6.0);
        });
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
) -> MultiSeriesPlotResponse {
    type SharedPlotCache = Arc<Mutex<MultiSeriesPlotCache>>;
    let cache_id = options.id.with("multi_series_plot_points");
    let cache: SharedPlotCache = crate::scene_state::scene_data(ui.ctx(), |data| {
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
    if options.show_legend { plot = plot.legend(Legend::default()); }
    plot = plot.show_x(options.show_hover_values).show_y(options.show_hover_values);
    if options.simplified_grid || options.compact_layout { plot = simplify_grid(plot); }
    if options.compact_layout { plot = plot.show_axes(false); }
    if !options.compact_layout { render_plot_caption(ui, options.caption.as_deref(), options.text_size); }
    if options.reset_bounds {
        plot = plot.reset();
    }
    if options.log_y {
        plot = plot.y_axis_formatter(|mark, _range| crate::plot_fmt::log_y_tick(mark.value));
    }
    if let Some(label) = options.y_axis_label.as_deref().filter(|_| !options.compact_layout) {
        plot = plot.y_axis_label(label.to_owned());
    }
    if let Some(label) = options.x_axis_label.as_deref().filter(|_| !options.compact_layout) {
        plot = plot.x_axis_label(label.to_owned());
    }
    if let Some(range) = options.y_reference_range {
        for value in range {
            if !options.log_y {
                plot = plot.include_y(value);
            } else if value > 0.0 {
                plot = plot.include_y(value.log10());
            }
        }
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
            // With one trace there is no run identity to encode as a dash.
            let style = if item_count == 1 { LineStyle::Solid } else { match line.style {
                MultiSeriesStyle::Solid => LineStyle::Solid,
                MultiSeriesStyle::Dashed => LineStyle::dashed_dense(),
                MultiSeriesStyle::Dotted => LineStyle::dotted_dense(),
                MultiSeriesStyle::DashDot => LineStyle::dashed_loose(),
            }};
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
    let endpoints = series
        .iter()
        .map(|line| &line.points)
        .chain(overlays.iter().map(|line| &line.points))
        .filter_map(|points| Some([points.first()?[0], points.last()?[0]]))
        .fold(None::<[f64; 2]>, |span, bounds| {
            Some(span.map_or(bounds, |span| {
                [span[0].min(bounds[0]), span[1].max(bounds[1])]
            }))
        });
    let response = with_plot_text(ui, options.text_size, |ui| {
        plot.show(ui, |plot_ui| {
            for (label, color, style, points) in &plotted_series {
                add_cached_line(
                    plot_ui,
                    Line::new(label.clone(), PlotPoints::from(points.points.as_slice()))
                        .color(*color)
                        .width(options.line_width)
                        .style(*style),
                    points.bounds,
                    true,
                );
            }
            for (label, color, points) in &plotted_overlays {
                add_cached_line(
                    plot_ui,
                    Line::new(label.clone(), PlotPoints::from(points.points.as_slice()))
                        .color(*color)
                        .width(options.line_width),
                    points.bounds,
                    true,
                );
            }
            follow_latest_x(
                plot_ui,
                &egui_ctx,
                options.id,
                endpoints,
                options.follow_latest,
                options.time_window_s,
                options.time_window_revision,
            );
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
        })
    });
    if options.compact_layout {
        render_compact_labels(ui, &response.transform,
            options.x_axis_label.as_deref(), options.y_axis_label.as_deref(), options.caption.as_deref(), options.text_size, options.log_y,
            (options.show_latest_value && item_count==1).then(|| series.first()
                .and_then(|line|line.points.last()).or_else(||overlays.first().and_then(|line|line.points.last()))
                .map(|point| point[1])).flatten());
    }
    MultiSeriesPlotResponse {
        clicked_x,
        time_span_s: response.transform.bounds().width(),
        response: response.response,
    }
}

#[cfg(test)]
mod follow_tests {
    use super::{follow_latest_x, following_bounds};
    use crate::shared_time_axis::shared_time_span;
    use egui_plot::{Line, Plot, PlotBounds, PlotMemory};

    #[test]
    fn shared_time_axes_align_late_start_and_idle_histories() {
        // A sensor starts after settling; another channel retains setup and
        // stops publishing earlier. Neither is a different time origin.
        let spans = [[2.016_666_7, 6.1], [0.0, 3.0]];
        let shared = shared_time_span(spans).unwrap();
        assert_eq!(shared, [0.0, 6.1]);
        let contexts = [egui::Context::default(), egui::Context::default()];
        for context in &contexts {
            let (_, bounds) = frame(context, Some(shared), true, Some(5.0));
            assert!((bounds.min()[0] - 1.1).abs() < 1e-9);
            assert!((bounds.max()[0] - 6.1).abs() < 1e-9);
        }
        // Independent views keep their original behavior when not grouped.
        assert_ne!(
            following_bounds(2.016_666_7, 6.1, 5.0),
            following_bounds(0.0, 3.0, 5.0)
        );
        assert_eq!(shared_time_span([]), None);
    }

    fn frame(
        ctx: &egui::Context,
        data: Option<[f64; 2]>,
        follow: bool,
        window: Option<f64>,
    ) -> (egui::Id, PlotBounds) {
        frame_with_revision(ctx, data, follow, window, 0)
    }

    fn frame_with_revision(
        ctx: &egui::Context,
        data: Option<[f64; 2]>,
        follow: bool,
        window: Option<f64>,
        revision: u64,
    ) -> (egui::Id, PlotBounds) {
        let mut result = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 400.0),
                )),
                ..Default::default()
            },
            |ui| {
                let id = egui::Id::new("follow-test");
                let response = Plot::new("follow-test").id(id).show(ui, |plot_ui| {
                    if let Some([first, last]) = data {
                        plot_ui.line(Line::new("data", vec![[first, 0.0], [last, 1.0]]));
                    }
                    follow_latest_x(
                        plot_ui,
                        ctx,
                        egui::Id::new("follow-test"),
                        data,
                        follow,
                        window,
                        revision,
                    );
                });
                result = Some((id, *response.transform.bounds()));
            },
        );
        output.textures_delta.clear();
        result.unwrap()
    }

    fn assert_x(bounds: PlotBounds, expected: [f64; 2]) {
        assert!((bounds.min()[0] - expected[0]).abs() < 1e-9, "{bounds:?}");
        assert!((bounds.max()[0] - expected[1]).abs() < 1e-9, "{bounds:?}");
    }

    #[test]
    fn following_waits_for_data_keeps_user_zoom_and_restores_auto_fit() {
        let ctx = egui::Context::default();
        frame(&ctx, None, true, Some(5.0));
        assert_x(frame(&ctx, Some([0.0, 1.0]), true, Some(5.0)).1, [0.0, 5.0]);
        let (id, bounds) = frame(&ctx, Some([0.0, 8.0]), true, Some(5.0));
        assert_x(bounds, [3.0, 8.0]);

        // The same plot memory is changed by egui's user zoom interaction.
        let mut memory = PlotMemory::load(&ctx, id).unwrap();
        let mut zoomed = *memory.bounds();
        zoomed.set_x(&PlotBounds::from_min_max([6.0, 0.0], [8.0, 1.0]));
        memory.set_bounds(zoomed);
        memory.store(&ctx, id);
        assert_x(
            frame(&ctx, Some([0.0, 10.0]), true, Some(5.0)).1,
            [8.0, 10.0],
        );
        // Clearing the numeric request retains the current, zoomed span.
        assert_x(frame(&ctx, Some([0.0, 12.0]), true, None).1, [10.0, 12.0]);
        let (_, fitted) = frame(&ctx, Some([0.0, 12.0]), false, None);
        assert!(fitted.min()[0] <= 0.0 && fitted.max()[0] >= 12.0);
        let (id, _) = frame(&ctx, Some([0.0, 12.0]), true, Some(5.0));
        frame(&ctx, Some([0.0, 12.0]), false, Some(5.0));
        let mut memory = PlotMemory::load(&ctx, id).unwrap();
        memory.auto_bounds.x = false;
        let mut zoomed = *memory.bounds();
        zoomed.set_x(&PlotBounds::from_min_max([10.0, 0.0], [12.0, 1.0]));
        memory.set_bounds(zoomed);
        memory.store(&ctx, id);
        assert_x(
            frame(&ctx, Some([0.0, 14.0]), true, Some(5.0)).1,
            [12.0, 14.0],
        );
        // An explicit repeat of the saved five-second request must work even
        // after the view was zoomed to two seconds.
        assert_x(
            frame_with_revision(&ctx, Some([0.0, 16.0]), true, Some(5.0), 1).1,
            [11.0, 16.0],
        );
    }
    #[test]
    fn compact_labels_are_horizontal_inside_the_native_plot_frame() {
        let ctx = egui::Context::default();
        let mut frame = None;
        let mut transform = None;
        let mut output = ctx.run_ui(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(880.0, 400.0))),
            ..Default::default()
        }, |ui| {
            let response = Plot::new("compact-labels").show_axes(false).show(ui, |plot| {
                plot.line(Line::new("native samples", vec![[0.0,20.0],[5.0,-107.0]]));
            });
            frame = Some(*response.transform.frame());
            transform = Some(response.transform.clone());
            super::render_compact_labels(ui, &response.transform, Some("Time (s)"),
                Some("°C"), Some("Air temperature"), Some(28.0), false, Some(-107.4));
        });
        output.textures_delta.clear();
        let frame = frame.unwrap();
        for label in ["°C", "Time (s)", "Air temperature", "-107.4 °C"] {
            let text = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => Some(text), _ => None,
            }).unwrap();
            assert_eq!(text.angle, 0.0);
            assert!(frame.contains_rect(text.visual_bounding_rect()), "{label} must stay inside the native frame");
        }
        let transform = transform.unwrap();
        for (label, axis, value) in [("2", 0, 2.0), ("-50", 1, -50.0)] {
            let text = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => Some(text), _ => None,
            }).expect("readable native tick must be painted");
            let center = text.pos + text.galley.size() * 0.5;
            let expected = transform.position_from_point(&egui_plot::PlotPoint::new(value,value));
            assert!((center[axis]-expected[axis]).abs()<0.01);
            assert!(frame.contains_rect(text.visual_bounding_rect()));
        }
    }

    #[test]
    fn simplified_grid_has_only_readable_major_marks() {
        let marks = super::major_grid_marks(egui_plot::GridInput { bounds: (-140.0, 30.0), base_step_size: 1.0 });
        assert_eq!(marks.iter().map(|m| m.value).collect::<Vec<_>>(), vec![-100.0, -50.0, 0.0]);
        assert!(super::major_grid_marks(egui_plot::GridInput { bounds: (0.0, f64::INFINITY), base_step_size: 1.0 }).is_empty());
    }
    #[test]
    fn following_preserves_span_before_and_after_the_window_fills() {
        assert_eq!(following_bounds(0.0, 1.0, 5.0), Some([0.0, 5.0]));
        assert_eq!(following_bounds(0.0, 8.0, 5.0), Some([3.0, 8.0]));
        assert_eq!(following_bounds(100.0, 108.0, 2.0), Some([106.0, 108.0]));
        assert_eq!(following_bounds(0.0, 8.0, 0.0), None);
        assert_eq!(following_bounds(0.0, f64::NAN, 5.0), None);
    }
}
