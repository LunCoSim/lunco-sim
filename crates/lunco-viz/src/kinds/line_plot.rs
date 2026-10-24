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
//! populated from the `SignalRegistry` when opened. Picker entries are
//! cached by catalog revision while labels continue to use live owner names.
//! Without this toolbar, new
//! plots appeared frozen to users because the only signal-picker
//! (Telemetry's checkboxes) targeted the default plot exclusively.
//!
//! Style is stored in `VisualizationConfig.style` as
//! [`LinePlotStyle`] (serde JSON) so the choice survives save/reload.

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use egui;
use egui_plot::{Corner, Legend, Line, Plot, PlotPoints};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::collections::HashMap;

use crate::registry::{VisualizationRegistry, VizFitRequests};
use crate::signal::{
    PersistedSignalRef, ScalarHistory, ScalarHistorySnapshot, ScalarSample, SignalMeta, SignalRef,
    SignalRegistry, SignalType,
};
use crate::view::{Panel2DCtx, ViewKind};
use crate::viz::{RoleSpec, SignalBinding, Visualization, VisualizationConfig, VizKindId};
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{Command, GlobalEntityId, on_command, register_commands};
use lunco_workbench_core::PanelCtx;

/// A line-plot toolbar edit requested by the panel's read-only paint pass.
#[derive(Event)]
pub(crate) struct LinePlotEditRequested {
    pub(crate) viz: lunco_viz_core::VizId,
    edit: Edit,
}

/// A one-shot request to reset a plot's egui bounds on its next render.
#[derive(Event, Clone, Copy)]
pub(crate) struct LinePlotFitRequested {
    pub(crate) viz: lunco_viz_core::VizId,
}

/// LinePlot-specific options stashed in
/// [`VisualizationConfig::style`]. Serialised as JSON for on-disk
/// round-trip. All fields optional — missing fields keep default
/// behaviour (X = time, auto-labeled axes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinePlotStyle {
    /// Signal to drive the X axis. `None` = simulation time (the
    /// classic time-series plot). When `Some`, each Y sample is
    /// paired with the X sample at the same (or nearest-earlier)
    /// time, producing a phase-space trajectory.
    #[serde(default)]
    pub x_signal: Option<SignalRef>,
    /// Stable counterpart of `x_signal`, maintained by the viz lifecycle so a
    /// phase-space plot survives the same scene reload as its Y bindings.
    #[serde(default)]
    pub x_persisted_signal: Option<PersistedSignalRef>,
    /// Optional authored axis labels. When `None`, the shared signal naming
    /// policy is used by controls, legends, and hover text; no raw generated
    /// name is inserted into an axis-title row.
    #[serde(default)]
    pub x_label: Option<String>,
    #[serde(default)]
    pub y_label: Option<String>,
    /// Values kept visible during automatic scaling; data may expand beyond
    /// this interval. Prevents a steady signal magnifying floating-point noise.
    #[serde(default)]
    pub y_reference_range: Option<[f64; 2]>,
    /// Plot the Y axis on a log10 scale. Values ≤ 0 are dropped (gaps,
    /// not clamped). Persists with the plot like the other style.
    #[serde(default)]
    pub log_y: bool,
    /// Curve thickness in logical display pixels; independent of UI zoom.
    #[serde(default = "default_line_width")]
    pub line_width: f32,
    /// Optional authored colour for this plot, including experiment curves.
    /// Individual live binding colours take precedence.
    #[serde(default)]
    pub series_color: Option<[u8; 3]>,
    /// Plot ticks, titles, legend and hover text in logical pixels. None
    /// inherits the theme without changing surrounding UI controls.
    #[serde(default)]
    pub text_size: Option<f32>,
    /// Show signal pickers and editing buttons. The plot context menu remains
    /// available when these controls are hidden for a presentation.
    #[serde(default = "default_show_controls")]
    pub show_controls: bool,
    /// Show pointer crosshairs and measured-value tooltips. Independent of axes
    /// and editing controls, so presentation plots can stay unobstructed.
    #[serde(default = "default_show_controls")]
    pub show_hover_values: bool,
    /// Curve identity can be omitted when the axes already explain one series.
    #[serde(default = "default_show_controls")]
    pub show_legend: bool,
    /// Sparse major guides instead of a dense engineering grid.
    #[serde(default)]
    pub simplified_grid: bool,
    /// Title and horizontal quantity/unit labels inside the plot frame.
    #[serde(default)]
    pub compact_layout: bool,
    /// Show the latest measured value when exactly one trace is displayed.
    #[serde(default)]
    pub show_latest_value: bool,
    /// Human-readable explanation above the plot, independent of signal names.
    #[serde(default)]
    pub caption: Option<String>,
    /// Scroll with new time samples while retaining the current visible span.
    #[serde(default)]
    pub follow_latest: bool,
    /// Optional initial/requested time span. Following retains later user zoom.
    #[serde(default)]
    pub time_window_s: Option<f64>,
    /// Explicit window requests are repeatable after the user zooms the view.
    #[serde(default)]
    pub time_window_revision: u64,
}

fn default_show_controls() -> bool {
    true
}

fn default_line_width() -> f32 {
    1.5
}

impl Default for LinePlotStyle {
    fn default() -> Self {
        Self {
            x_signal: None,
            x_persisted_signal: None,
            x_label: None,
            y_label: None,
            y_reference_range: None,
            log_y: false,
            line_width: default_line_width(),
            series_color: None,
            text_size: None,
            show_controls: true,
            show_hover_values: true,
            show_legend: true,
            simplified_grid: false,
            compact_layout: false,
            show_latest_value: false,
            caption: None,
            follow_latest: false,
            time_window_s: None,
            time_window_revision: 0,
        }
    }
}

impl LinePlotStyle {
    pub(crate) fn load(config: &VisualizationConfig) -> Self {
        serde_json::from_value(config.style.clone()).unwrap_or_default()
    }
    /// Render-path loader: parse the style JSON once and cache the
    /// result in egui context data, keyed by the blob itself. The blob
    /// only changes on a user edit, so steady-state frames cost a
    /// `Value` equality check instead of two serde deserializations
    /// per plot per frame (toolbar + body both call this).
    pub fn load_cached(ctx: &egui::Context, config: &VisualizationConfig) -> Self {
        let id = egui::Id::new(("line_plot_style", config.id.raw()));
        if let Some(cached) =
            ctx.data(|d| d.get_temp::<std::sync::Arc<(serde_json::Value, LinePlotStyle)>>(id))
        {
            if cached.0 == config.style {
                return cached.1.clone();
            }
        }
        let parsed = Self::load(config);
        ctx.data_mut(|d| {
            d.insert_temp(
                id,
                std::sync::Arc::new((config.style.clone(), parsed.clone())),
            );
        });
        parsed
    }
    pub(crate) fn save(&self, config: &mut VisualizationConfig) {
        config.style = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
    }
}

/// Operator-facing identity for an entity-scoped signal.  `SignalRef::path`
/// alone is intentionally not unique — four wheels can all publish
/// `axle_torque` — so every graph surface shows the owning USD prim as well.
/// The full path remains available in telemetry tooltips and persistence keeps
/// the entity identity; this is solely the concise plot presentation.
fn component_parameter_label(wb: &PanelCtx, signal: &SignalRef) -> String {
    let meta = wb
        .resource::<SignalRegistry>()
        .and_then(|registry| registry.meta(signal));
    component_parameter_label_with_meta(wb, signal, meta)
}

fn component_parameter_label_with_meta(
    wb: &PanelCtx,
    signal: &SignalRef,
    meta: Option<&SignalMeta>,
) -> String {
    let owner = wb
        .get::<Name>(signal.entity)
        .map(|name| {
            lunco_core::entity_display_name(
                Some(name),
                wb.get::<lunco_core::markers::Callsign>(signal.entity),
                wb.get::<lunco_core::CatalogEntryId>(signal.entity),
            )
        })
        .unwrap_or_else(|| "Unnamed entity".to_string());
    let channel = crate::signal::display_channel_label(
        &signal.path,
        meta.and_then(|metadata| metadata.group_path.as_deref()),
        meta.and_then(|metadata| metadata.unit.as_deref()),
        wb.resource_expect::<crate::telemetry_browser::TelemetryDisplaySettings>()
            .show_generated_names,
    );
    if meta
        .and_then(|metadata| metadata.group_path.as_deref())
        .is_some()
    {
        format!("{owner} · {channel}")
    } else {
        format!("[{owner}.{channel}]")
    }
}

/// Operator-facing label for a binding, with its unit appended when the
/// registry carries one. `meta` is the already-resolved registry metadata for
/// `binding.source`; callers that render many chips per frame resolve it once
/// instead of asking `binding_label` to re-query the registry on every call.
fn binding_label(wb: &PanelCtx, binding: &SignalBinding, meta: Option<&SignalMeta>) -> String {
    let label = binding
        .label
        .clone()
        .unwrap_or_else(|| component_parameter_label_with_meta(wb, &binding.source, meta));
    let unit = meta
        .and_then(|m| m.unit.as_deref())
        .filter(|unit| !unit.is_empty() && *unit != "1");
    unit.map_or_else(|| label.clone(), |unit| format!("{label} [{unit}]"))
}

/// Human-facing metadata shown wherever a signal can be selected.  Keep the
/// signal identity in the label and put authored documentation in the hint so
/// long Modelica/USD prose does not make the graph toolbar unusable.
/// `meta` is the already-resolved registry metadata for `signal`.
fn signal_hint(meta: Option<&SignalMeta>) -> Option<String> {
    let meta = meta?;
    let mut parts = Vec::new();
    if let Some(unit) = meta.unit.as_deref().filter(|u| !u.is_empty()) {
        parts.push(format!("unit: {unit}"));
    }
    if let Some(description) = meta.description.as_deref().filter(|d| !d.is_empty()) {
        parts.push(description.to_owned());
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

#[derive(Debug)]
struct ToolbarSignal {
    source: SignalRef,
    meta: Option<SignalMeta>,
    hint: Option<String>,
}

#[derive(Debug)]
struct ToolbarSignalCatalog {
    registry_address: usize,
    revision: u64,
    signals: Vec<ToolbarSignal>,
}

/// Build the signal-picker entries only when a picker is open, then reuse them
/// until the signal catalog changes.
fn toolbar_signal_catalog(
    ctx: &egui::Context,
    registry: &SignalRegistry,
) -> std::sync::Arc<ToolbarSignalCatalog> {
    let registry_address = registry as *const SignalRegistry as usize;
    let revision = registry.catalog_revision();
    let cache_id = egui::Id::new("line_plot_toolbar_signal_catalog");
    type CachedCatalog = std::sync::Arc<ToolbarSignalCatalog>;
    if let Some(cached) =
        crate::scene_state::scene_data(ctx, |data| data.get_temp::<CachedCatalog>(cache_id))
    {
        if cached.registry_address == registry_address && cached.revision == revision {
            return cached;
        }
    }

    let signals = registry
        .iter_signals()
        .filter(|&(_, kind)| kind == SignalType::Scalar)
        .map(|(signal, _)| {
            let meta = registry.meta(signal).cloned();
            ToolbarSignal {
                source: signal.clone(),
                hint: signal_hint(meta.as_ref()),
                meta,
            }
        })
        .collect();
    let catalog = std::sync::Arc::new(ToolbarSignalCatalog {
        registry_address,
        revision,
        signals,
    });
    crate::scene_state::scene_data(ctx, |data| data.insert_temp(cache_id, catalog.clone()));
    catalog
}

/// Keep the optional phase-space X axis on the same stable-identity lifecycle
/// as normal `SignalBinding`s. A missing source remains absent until a matching
/// content entity returns; it is never rebound by mnemonic alone.
pub(crate) fn reconcile_persisted_x_source(
    config: &mut VisualizationConfig,
    gid_of: &HashMap<Entity, GlobalEntityId>,
    entity_of: &HashMap<GlobalEntityId, Entity>,
) {
    let mut style = LinePlotStyle::load(config);
    let Some(x) = style.x_signal.clone() else {
        return;
    };
    let mut changed = false;
    if style.x_persisted_signal.is_none() {
        let persisted = x.to_persisted(|entity| gid_of.get(&entity).copied());
        if persisted.is_some() {
            style.x_persisted_signal = persisted;
            changed = true;
        }
    }
    if let Some(persisted) = &style.x_persisted_signal {
        if let Some(source) = persisted.resolve(|gid| entity_of.get(&gid).copied()) {
            if style.x_signal.as_ref() != Some(&source) {
                style.x_signal = Some(source);
                changed = true;
            }
        }
    }
    if changed {
        style.save(config);
    }
}

/// Change-detector for one ring buffer: length + first/last sample
/// times. Histories are append-only with monotone time, so any push,
/// eviction, or clear moves at least one component. Cheap enough to
/// compute every frame; equality means "reuse the tessellated points".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HistFingerprint {
    len: usize,
    first_t: u64,
    last_t: u64,
}

fn hist_fingerprint(h: &crate::signal::ScalarHistory) -> HistFingerprint {
    HistFingerprint {
        len: h.len(),
        first_t: h.front().map_or(0, |s| s.time.to_bits()),
        last_t: h.back().map_or(0, |s| s.time.to_bits()),
    }
}

#[derive(Default)]
struct ScalarHistoryPointsCache {
    displayed: Option<(HistFingerprint, std::sync::Arc<Vec<[f64; 2]>>)>,
    build: Option<Task<(HistFingerprint, std::sync::Arc<Vec<[f64; 2]>>)>>,
    last_build_sec: Option<f64>,
}

/// Return the last completed time-series snapshot, rebuilding only when the
/// source history changes. Plot hosts that combine live curves with another
/// surface keep full-history flattening off the UI thread.
pub fn cached_scalar_history_points(
    ctx: &egui::Context,
    registry: &SignalRegistry,
    source: &SignalRef,
) -> Option<std::sync::Arc<Vec<[f64; 2]>>> {
    let history = registry.scalar_history(source)?;
    if history.is_empty() {
        return None;
    }
    let key = hist_fingerprint(history);
    let cache_id = egui::Id::new(("line_plot_scalar_history", source));
    type SharedCache = std::sync::Arc<std::sync::Mutex<ScalarHistoryPointsCache>>;
    let cache: SharedCache = crate::scene_state::scene_data(ctx, |data| {
        if let Some(existing) = data.get_temp::<SharedCache>(cache_id) {
            existing
        } else {
            let fresh = SharedCache::default();
            data.insert_temp(cache_id, fresh.clone());
            fresh
        }
    });

    let now = ctx.input(|input| input.time);
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let completed = cache
        .build
        .as_mut()
        .and_then(|task| future::block_on(future::poll_once(task)));
    if let Some(snapshot) = completed {
        cache.build = None;
        cache.displayed = Some(snapshot);
    }

    let data_matches = cache
        .displayed
        .as_ref()
        .is_some_and(|(fingerprint, _)| *fingerprint == key);
    let refresh_due = cache
        .last_build_sec
        .is_none_or(|last| now - last >= PLOT_SERIES_REFRESH_INTERVAL_SEC);
    if !data_matches && cache.build.is_none() && (cache.displayed.is_none() || refresh_due) {
        let sample_count = history.len();
        let snapshot_span = bevy::log::info_span!(
            "line_plot_scalar_history_snapshot",
            signal = ?source,
            samples = sample_count
        );
        let snapshot = snapshot_span.in_scope(|| history.snapshot());
        let build_span = bevy::log::info_span!(
            "line_plot_scalar_history_build_worker",
            signal = ?source,
            samples = sample_count,
            output_points = bevy::log::tracing::field::Empty
        );
        cache.last_build_sec = Some(now);
        cache.build = Some(AsyncComputeTaskPool::get().spawn(async move {
            let _build_span = build_span.enter();
            let points: Vec<[f64; 2]> = snapshot
                .into_samples()
                .into_iter()
                .map(|sample| [sample.time, sample.value])
                .collect();
            build_span.record("output_points", points.len());
            (key, std::sync::Arc::new(points))
        }));
    }
    if cache.build.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
    cache.displayed.as_ref().map(|(_, points)| points.clone())
}

/// Everything a cached tessellation depends on. Stored next to the
/// points; a mismatch on any component forces a rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SeriesKey {
    y: HistFingerprint,
    /// Fingerprint of the X signal's history in phase-space mode.
    x: Option<HistFingerprint>,
    /// X identity matters even when two histories currently share a fingerprint.
    x_signal: Option<SignalRef>,
    log_y: bool,
    /// Plot pixel width bucket — decimation depth depends on it.
    px_w: u32,
}

type PlotSeriesPoints =
    std::sync::Arc<(SeriesKey, Vec<egui_plot::PlotPoint>, egui_plot::PlotBounds)>;

#[derive(Default)]
struct PlotSeriesCache {
    displayed: Option<PlotSeriesPoints>,
    build: Option<Task<PlotSeriesPoints>>,
    last_build_sec: Option<f64>,
}

const PLOT_SERIES_REFRESH_INTERVAL_SEC: f64 = 1.0 / 20.0;

fn same_series_presentation(left: &SeriesKey, right: &SeriesKey) -> bool {
    left.x_signal == right.x_signal && left.log_y == right.log_y && left.px_w == right.px_w
}

/// Reuse the last plot buffer while a worker builds a newer history snapshot.
/// Snapshot copying is rate-limited and min-max decimation never runs in the
/// UI frame; at most one build is active for each plot binding.
fn cached_plot_series_points(
    ctx: &egui::Context,
    cache_id: egui::Id,
    key: SeriesKey,
    y_history: &crate::signal::ScalarHistory,
    x_history: Option<&crate::signal::ScalarHistory>,
    pixel_width: f32,
) -> Option<PlotSeriesPoints> {
    type SharedCache = std::sync::Arc<std::sync::Mutex<PlotSeriesCache>>;
    let cache: SharedCache = crate::scene_state::scene_data(ctx, |data| {
        if let Some(existing) = data.get_temp::<SharedCache>(cache_id) {
            existing
        } else {
            let fresh = SharedCache::default();
            data.insert_temp(cache_id, fresh.clone());
            fresh
        }
    });

    let now = ctx.input(|input| input.time);
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let completed = cache
        .build
        .as_mut()
        .and_then(|task| future::block_on(future::poll_once(task)));
    if let Some(points) = completed {
        cache.build = None;
        cache.displayed = Some(points);
    }

    let display_matches = cache
        .displayed
        .as_ref()
        .is_some_and(|points| same_series_presentation(&points.0, &key));
    let data_matches = cache
        .displayed
        .as_ref()
        .is_some_and(|points| points.0 == key);
    let refresh_due = cache
        .last_build_sec
        .is_none_or(|last| now - last >= PLOT_SERIES_REFRESH_INTERVAL_SEC);

    if !data_matches && cache.build.is_none() && (!display_matches || refresh_due) {
        let y_sample_count = y_history.len();
        let x_sample_count = x_history.map_or(0, ScalarHistory::len);
        let snapshot_span = bevy::log::info_span!(
            "line_plot_history_snapshot",
            plot_cache = ?cache_id,
            y_samples = y_sample_count,
            x_samples = x_sample_count
        );
        let (y_samples, x_samples) = snapshot_span
            .in_scope(|| (y_history.snapshot(), x_history.map(ScalarHistory::snapshot)));
        let build_key = key.clone();
        let build_span = bevy::log::info_span!(
            "line_plot_series_build_worker",
            plot_cache = ?cache_id,
            y_samples = y_sample_count,
            x_samples = x_sample_count,
            pixel_width = pixel_width as u32,
            output_points = bevy::log::tracing::field::Empty
        );
        cache.last_build_sec = Some(now);
        cache.build = Some(AsyncComputeTaskPool::get().spawn(async move {
            let _build_span = build_span.enter();
            let y_samples = y_samples.into_samples();
            let x_samples = x_samples.map(ScalarHistorySnapshot::into_samples);
            let time_on_x = x_samples.is_none();
            let mut points = match x_samples {
                Some(xs) => pair_by_time(&xs, y_samples),
                None => y_samples
                    .iter()
                    .map(|sample| [sample.time, sample.value])
                    .collect(),
            };
            if build_key.log_y {
                points = crate::plot_fmt::log_y_points(&points);
            }
            let mut bounds = egui_plot::PlotBounds::NOTHING;
            for point in &points {
                bounds.extend_with(&egui_plot::PlotPoint::from(*point));
            }
            if time_on_x {
                // Min-max pairs cover two horizontal points per bin, giving
                // the rendered line about one sample per display point.
                if let Some(decimated) =
                    crate::plot_fmt::decimate_min_max(&points, pixel_width * 0.5)
                {
                    points = decimated;
                }
            }
            let output_point_count = points.len();
            let points: Vec<_> = points.into_iter().map(egui_plot::PlotPoint::from).collect();
            build_span.record("output_points", output_point_count);
            std::sync::Arc::new((build_key, points, bounds))
        }));
    }

    cache
        .displayed
        .as_ref()
        .filter(|points| same_series_presentation(&points.0, &key))
        .cloned()
}

pub const LINE_PLOT_KIND: VizKindId = VizKindId::new_static("line_plot");

const ROLE_Y: RoleSpec = RoleSpec {
    role: "y",
    accepted_types: &[SignalType::Scalar],
    single: false,
};

#[derive(Default)]
pub struct LinePlot;

impl Visualization for LinePlot {
    fn kind_id(&self) -> VizKindId {
        LINE_PLOT_KIND
    }
    fn display_name(&self) -> &'static str {
        "Line plot (time-series)"
    }
    fn role_schema(&self) -> &'static [RoleSpec] {
        &[ROLE_Y]
    }
    fn compatible_views(&self) -> &'static [ViewKind] {
        &[ViewKind::Panel2D]
    }

    fn render_panel_2d(&self, ctx: &mut Panel2DCtx, config: &VisualizationConfig) {
        self.render_panel_2d_with_actions(ctx, config, |_, _| {});
    }
}

impl LinePlot {
    /// Render a line plot with optional controls appended to its single
    /// compact toolbar row. Plot-specific actions (new/duplicate/export) are
    /// supplied by the host application, while signal selection remains owned
    /// by this visualization so every line plot has the same editing surface.
    pub fn render_panel_2d_with_actions(
        &self,
        ctx: &mut Panel2DCtx,
        config: &VisualizationConfig,
        actions: impl FnOnce(&mut egui::Ui, &mut PanelCtx),
    ) {
        // Series colours come from the active THEME (`PlotTokens::series`), published
        // into the egui data cache once per frame — plots re-theme like everything else.
        let theme = lunco_theme::active(ctx.ui.ctx());
        // Toolbar first — lets the user edit bindings even before
        // any signal data arrives. Returns the mutation the user
        // requested, which we apply after releasing the read borrow
        // on the registry.
        let style = LinePlotStyle::load_cached(ctx.ui.ctx(), config);
        let edit = if style.show_controls {
            render_toolbar(ctx, config, actions)
        } else {
            None
        };
        if let Some(edit) = edit {
            // Queue the registry mutation for after the egui pass —
            // render holds only read access to the world.
            let id = config.id;
            ctx.wb.trigger(LinePlotEditRequested { viz: id, edit });
            // Don't render the body this frame — the config just
            // changed. Next frame picks up the new bindings.
            return;
        }

        let registry = match ctx.wb.resource::<SignalRegistry>() {
            Some(r) => r,
            None => {
                ctx.ui.label("SignalRegistry not installed.");
                return;
            }
        };

        // Collect `y`-role bindings. Filter hidden + missing-signal
        // bindings here so the legend shows the same set as the plot.
        let y_bindings: SmallVec<[&SignalBinding; 8]> = config
            .inputs
            .iter()
            .filter(|b| b.role == ROLE_Y.role && b.visible)
            .collect();

        if y_bindings.is_empty() {
            let (text, muted) = ctx
                .wb
                .resource::<lunco_theme::Theme>()
                .map(|t| (t.tokens.text, t.tokens.text_subdued))
                .unwrap_or((egui::Color32::GRAY, egui::Color32::DARK_GRAY));
            ctx.ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                ui.label(egui::RichText::new("No signals bound.").color(text));
                ui.label(
                    egui::RichText::new(
                        "Add one from the ➕ picker above, or drag a \
                         variable from Telemetry.",
                    )
                    .color(muted),
                );
            });
            return;
        }

        // A scene replacement removes the previous entity-scoped histories
        // before the replacement source has emitted. Do not render that as a
        // blank chart: tell the participant exactly what is happening and keep
        // the binding alive for the lifecycle reconciler to restore.
        let unavailable: SmallVec<[String; 8]> = y_bindings
            .iter()
            .filter(|b| {
                registry
                    .scalar_history(&b.source)
                    .is_none_or(|history| history.is_empty())
            })
            .map(|b| {
                let meta = ctx
                    .wb
                    .resource::<SignalRegistry>()
                    .and_then(|r| r.meta(&b.source));
                binding_label(ctx.wb, b, meta)
            })
            .collect();

        // Plot pixel width — needed *before* tessellation because the
        // decimation depth (and thus the cache key) depends on it.
        // Taken after the toolbar so it matches the space `plot.show`
        // gets below.
        if !style.compact_layout {
            crate::multi_series_plot::render_plot_caption(
                ctx.ui,
                style.caption.as_deref(),
                style.text_size,
            );
        }
        let remaining = ctx.ui.available_size_before_wrap();

        // Resolve the X-axis source once. For classic time-series we
        // just use each Y sample's own `time`. For phase-space mode
        // we pull the X signal's history and pair by time below.
        // Fingerprint travels into each series' cache key so an X-side
        // change also dirties the pairing.
        let x_history = style
            .x_signal
            .as_ref()
            .and_then(|xs| registry.scalar_history(xs))
            .filter(|history| !history.is_empty());
        let x_fp = x_history.map(hist_fingerprint);

        // Snapshot point buffers and their current labels/colors before the
        // plot takes a long-lived borrow on `ctx.ui`.
        //
        // History changes rebuild the point buffer; plot width, X source, and
        // style changes also invalidate the per-binding cache.
        let mut series_to_plot: SmallVec<[(PlotSeriesPoints, String, egui::Color32); 8]> =
            y_bindings
                .iter()
                .filter_map(|b| {
                    let hist = registry.scalar_history(&b.source)?;
                    if hist.is_empty() {
                        return None;
                    }
                    let key = SeriesKey {
                        y: hist_fingerprint(hist),
                        x: x_fp,
                        x_signal: style.x_signal.clone(),
                        log_y: style.log_y,
                        px_w: remaining.x.max(1.0) as u32,
                    };
                    let cache_id =
                        egui::Id::new(("line_plot_async_series", config.id.raw())).with(&b.source);
                    let series = cached_plot_series_points(
                        ctx.ui.ctx(),
                        cache_id,
                        key,
                        hist,
                        x_history,
                        remaining.x,
                    )?;
                    if series.1.is_empty() {
                        return None;
                    }
                    // Entity identity is part of the visible label: four physical
                    // wheels may all expose `axle_torque` at once.
                    let meta = registry.meta(&b.source);
                    let label = binding_label(ctx.wb, b, meta);
                    let color = b
                        .color
                        .or_else(|| {
                            style
                                .series_color
                                .map(|[r, g, b]| egui::Color32::from_rgb(r, g, b))
                        })
                        .unwrap_or_else(|| crate::signal::color_for_signal(&theme, &b.source.path));
                    Some((series, label, color))
                })
                .collect();

        if series_to_plot.is_empty() {
            let muted = ctx
                .wb
                .resource::<lunco_theme::Theme>()
                .map(|t| t.tokens.text_subdued)
                .unwrap_or(egui::Color32::DARK_GRAY);
            let names = unavailable.join(", ");
            ctx.ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                ui.label(egui::RichText::new("Waiting for telemetry.").color(muted));
                ui.label(
                    egui::RichText::new(if names.is_empty() {
                        "The bound source has not emitted a sample yet.".to_string()
                    } else {
                        format!("Awaiting: {names}")
                    })
                    .color(muted),
                );
                ui.label(
                    egui::RichText::new(
                        "The plot will reconnect automatically when this scene publishes it.",
                    )
                    .color(muted),
                );
            });
            return;
        }

        if !unavailable.is_empty() {
            ctx.ui.label(
                egui::RichText::new(format!("Waiting for: {}", unavailable.join(", "))).color(
                    ctx.wb
                        .resource::<lunco_theme::Theme>()
                        .map(|t| t.tokens.text_subdued)
                        .unwrap_or(egui::Color32::DARK_GRAY),
                ),
            );
        }

        // Consume any pending Fit request for this viz. `auto_bounds`
        // alone only controls the *initial* policy — once the user
        // pans or zooms, egui_plot remembers their view and ignores
        // a policy change. `Plot::reset()` forces the plot to
        // discard stored memory and re-fit to the data exactly once.
        // Peek the pending fit now (read-only) so we can pass `.reset()`
        // to the plot this frame; clear the request via the typed observer
        // mutation so render stays a pure read.
        let fit_requested = ctx
            .wb
            .resource::<VizFitRequests>()
            .map(|r| r.is_pending(config.id))
            .unwrap_or(false);
        if fit_requested {
            let id = config.id;
            ctx.wb.trigger(LinePlotFitRequested { viz: id });
        }

        // The toolbar names the X source and every Y binding, while hover
        // labels carry the exact value and simulation time. Default axis-title
        // strings duplicate that information and consume a full plot row, so
        // only an explicit user-authored title is rendered.
        let x_label = style.x_label.clone();
        let y_label = style.y_label.clone();
        let log_y = style.log_y;

        // `remaining` (computed above, before tessellation) is the
        // space left after the toolbar + separator; `max_rect()` would
        // double-count that strip and push the plot off the bottom.
        let mut plot = Plot::new(("line_plot", config.id.raw()))
            .show_x(style.show_hover_values)
            .show_y(style.show_hover_values)
            .width(remaining.x)
            .height(remaining.y)
            .auto_bounds(egui::emath::Vec2b::new(true, true))
            // Hover any line → name + time + de-logged value.
            .label_formatter(move |pos| {
                // egui_plot 0.36 unified the (name, point) args into `HoverPosition`.
                let (name, point) = match pos {
                    egui_plot::HoverPosition::NearDataPoint {
                        plot_name,
                        position,
                        ..
                    } => (*plot_name, position),
                    egui_plot::HoverPosition::Elsewhere { position } => ("", position),
                };
                Some(crate::plot_fmt::hover_label(name, point, log_y))
            });
        let shared_time = ctx.time_axis.filter(|_| style.x_signal.is_none());
        if let Some(axis) = shared_time {
            plot = plot
                .link_axis(axis.group, [true, false])
                .link_cursor(axis.group, [true, false]);
        }
        if let Some(label) = x_label.filter(|_| !style.compact_layout) {
            plot = plot.x_axis_label(label);
        }
        if let Some(label) = y_label.filter(|_| !style.compact_layout) {
            plot = plot.y_axis_label(label);
        }
        if let Some([low, high]) = style.y_reference_range {
            if log_y {
                if low > 0.0 {
                    plot = plot.include_y(low.log10()).include_y(high.log10());
                }
            } else {
                plot = plot.include_y(low).include_y(high);
            }
        }
        if style.show_legend && y_bindings.len() > 1 {
            plot = plot.legend(
                Legend::default()
                    .position(Corner::RightTop)
                    .background_alpha(0.7),
            );
        }
        if log_y {
            // Grid marks live in log space; relabel them as real values.
            plot = plot.y_axis_formatter(|mark, _range| crate::plot_fmt::log_y_tick(mark.value));
        }
        if fit_requested {
            plot = plot.reset();
        }

        if style.compact_layout {
            plot = plot.show_axes(false);
        }
        if style.simplified_grid || style.compact_layout {
            plot = crate::multi_series_plot::simplify_grid(plot);
        }
        let latest_value = (style.show_latest_value && series_to_plot.len() == 1)
            .then(|| {
                series_to_plot[0]
                    .0
                    .1
                    .last()
                    .map(|point| if log_y { 10_f64.powf(point.y) } else { point.y })
            })
            .flatten();
        let endpoints = series_to_plot
            .iter()
            .map(|(series, _, _)| series.2)
            .filter(|bounds| bounds.is_finite_x())
            .fold(None::<[f64; 2]>, |span, bounds| {
                let bounds = [bounds.min()[0], bounds.max()[0]];
                Some(span.map_or(bounds, |span| {
                    [span[0].min(bounds[0]), span[1].max(bounds[1])]
                }))
            });
        let egui_ctx = ctx.ui.ctx().clone();
        let response = crate::multi_series_plot::with_plot_text(ctx.ui, style.text_size, |ui| {
            plot.show(ui, |plot_ui| {
                for (series, label, color) in &mut series_to_plot {
                    crate::multi_series_plot::add_cached_line(
                        plot_ui,
                        Line::new(std::mem::take(label), PlotPoints::from(series.1.as_slice()))
                            .color(*color)
                            .width(style.line_width),
                        series.2,
                        style.x_signal.is_none(),
                    );
                }
                crate::multi_series_plot::follow_latest_x(
                    plot_ui,
                    &egui_ctx,
                    egui::Id::new(("line_plot", config.id.raw())),
                    shared_time.map(|axis| axis.span_s).or(endpoints),
                    style.follow_latest && style.x_signal.is_none(),
                    style.time_window_s,
                    style.time_window_revision,
                );
            })
        });
        if style.compact_layout {
            crate::multi_series_plot::render_compact_labels(
                ctx.ui,
                &response.transform,
                style.x_label.as_deref(),
                style.y_label.as_deref(),
                style.caption.as_deref(),
                style.text_size,
                style.log_y,
                latest_value,
            );
        }
        response.response.context_menu(|ui| {
            render_appearance_menu(
                ui,
                ctx.wb,
                config.id.raw(),
                &style,
                response.transform.bounds().width(),
            );
        });
    }
}

/// One settings surface for live plots and experiment overlays, including
/// presentation views whose editing toolbar is hidden.
pub fn render_appearance_menu(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    plot: u64,
    style: &LinePlotStyle,
    current_time_span_s: f64,
) {
    ui.label("Plot appearance");
    let mut controls = style.show_controls;
    let mut x_label = style.x_label.clone().unwrap_or_default();
    let mut y_label = style.y_label.clone().unwrap_or_default();
    let mut request = ConfigureLinePlot {
        plot,
        ..Default::default()
    };
    if ui
        .checkbox(&mut controls, "Show editing controls")
        .changed()
    {
        request.show_controls = Some(controls);
    }
    let mut legend = style.show_legend;
    let mut hover = style.show_hover_values;
    if ui.checkbox(&mut hover, "Show hover values").changed() {
        request.show_hover_values = Some(hover);
    }
    if ui.checkbox(&mut legend, "Show curve legend").changed() {
        request.show_legend = Some(legend);
    }
    let mut grid = style.simplified_grid;
    if ui.checkbox(&mut grid, "Simplified grid").changed() {
        request.simplified_grid = Some(grid);
    }
    let mut latest = style.show_latest_value;
    if ui
        .checkbox(&mut latest, "Show latest single-trace value")
        .changed()
    {
        request.show_latest_value = Some(latest);
    }
    let mut inside = style.compact_layout;
    if ui
        .checkbox(&mut inside, "Compact title and horizontal axis labels")
        .changed()
    {
        request.compact_layout = Some(inside);
    }
    let mut caption = style.caption.clone().unwrap_or_default();
    ui.label("Title above graph");
    if ui.text_edit_singleline(&mut caption).changed() {
        request.caption = Some(caption);
    }
    ui.label("X axis title (include units)");
    if ui.text_edit_singleline(&mut x_label).changed() {
        request.x_label = Some(x_label);
    }
    ui.label("Y axis title (include units)");
    if ui.text_edit_singleline(&mut y_label).changed() {
        request.y_label = Some(y_label);
    }
    let mut use_range = style.y_reference_range.is_some();
    if ui
        .checkbox(&mut use_range, "Keep a reference Y range visible")
        .changed()
    {
        request.y_reference_range = Some(if use_range { vec![0.0, 1.0] } else { vec![] });
    }
    if let Some(mut range) = style.y_reference_range {
        let mut changed = false;
        ui.horizontal(|ui| {
            changed |= ui
                .add(egui::DragValue::new(&mut range[0]).prefix("Min "))
                .changed();
            changed |= ui
                .add(egui::DragValue::new(&mut range[1]).prefix("Max "))
                .changed();
        });
        if changed && range[0] < range[1] {
            request.y_reference_range = Some(range.to_vec());
        }
    }
    let mut width = style.line_width;
    if ui
        .add(
            egui::DragValue::new(&mut width)
                .range(0.5..=8.0)
                .prefix("Line width ")
                .suffix(" px"),
        )
        .changed()
    {
        request.line_width = Some(width);
    }
    let mut custom_text = style.text_size.is_some();
    if ui
        .checkbox(&mut custom_text, "Custom plot text size")
        .changed()
    {
        request.text_size = Some(if custom_text { 24.0 } else { 0.0 });
    }
    if let Some(mut size) = style.text_size {
        if ui
            .add(
                egui::DragValue::new(&mut size)
                    .range(8.0..=64.0)
                    .prefix("Text size ")
                    .suffix(" px"),
            )
            .changed()
        {
            request.text_size = Some(size);
        }
    }
    ui.separator();
    let mut follow = style.follow_latest;
    if ui.checkbox(&mut follow, "Follow latest data").on_hover_text("Keep the current time span and scroll as new samples arrive. Off restores fit-all history.").changed() {
                request.follow_latest = Some(follow);
            }
    let mut window = current_time_span_s;
    if ui.add_enabled(follow, egui::DragValue::new(&mut window).range(0.0..=86400.0).prefix("Time window ").suffix(" s"))
                .on_hover_text("Positive seconds sets a new visible span; zero follows with the current span. Later zoom changes are retained.").changed() {
                request.time_window_s = Some(window);
            }
    if request.show_legend.is_some()
        || request.simplified_grid.is_some()
        || request.compact_layout.is_some()
        || request.show_latest_value.is_some()
        || request.caption.is_some()
        || request.show_controls.is_some()
        || request.show_hover_values.is_some()
        || request.x_label.is_some()
        || request.y_label.is_some()
        || request.y_reference_range.is_some()
        || request.line_width.is_some()
        || request.follow_latest.is_some()
        || request.time_window_s.is_some()
        || request.text_size.is_some()
    {
        ctx.trigger(request);
    }
}

/// Configure a native plot through the same persisted style used by its UI.
/// Omitted fields retain their values; an empty title restores automatic labels.
#[Command(default)]
pub struct ConfigureLinePlot {
    pub plot: u64,
    pub x_label: Option<String>,
    pub y_label: Option<String>,
    /// Empty clears the reference range; otherwise exactly two increasing,
    /// finite values in the plotted signal's units. Autoscaling can expand it.
    pub y_reference_range: Option<Vec<f64>>,
    pub line_width: Option<f32>,
    /// Empty restores automatic colours; otherwise exactly three RGB bytes.
    pub series_color: Option<Vec<u8>>,
    /// 8–64 logical pixels for plot text only; zero restores the theme.
    pub text_size: Option<f32>,
    pub show_controls: Option<bool>,
    pub show_hover_values: Option<bool>,
    pub show_legend: Option<bool>,
    pub simplified_grid: Option<bool>,
    pub compact_layout: Option<bool>,
    pub show_latest_value: Option<bool>,
    /// Empty clears the explanatory caption.
    pub caption: Option<String>,
    pub follow_latest: Option<bool>,
    /// Positive seconds sets the requested window; zero restores current-window
    /// following. Omission leaves the setting unchanged.
    pub time_window_s: Option<f64>,
}

fn configure_line_plot(
    registry: &mut VisualizationRegistry,
    request: &ConfigureLinePlot,
) -> Result<(), String> {
    if request
        .series_color
        .as_ref()
        .is_some_and(|color| !color.is_empty() && color.len() != 3)
    {
        return Err("plot colour must be empty or three RGB bytes".into());
    }
    if request
        .text_size
        .is_some_and(|size| !size.is_finite() || (size != 0.0 && !(8.0..=64.0).contains(&size)))
    {
        return Err("plot text size must be zero (theme) or between 8 and 64 pixels".into());
    }
    if let Some(range) = &request.y_reference_range {
        if !range.is_empty()
            && (range.len() != 2 || !range.iter().all(|v| v.is_finite()) || range[0] >= range[1])
        {
            return Err("reference Y range must be empty or two increasing finite values".into());
        }
    }
    if request
        .line_width
        .is_some_and(|w| !w.is_finite() || !(0.5..=8.0).contains(&w))
    {
        return Err("line width must be finite and between 0.5 and 8 pixels".into());
    }
    if request
        .time_window_s
        .is_some_and(|window| !window.is_finite() || window < 0.0)
    {
        return Err(
            "time window must be finite and nonnegative; zero uses the current window".into(),
        );
    }
    let config = registry
        .get_mut(lunco_viz_core::VizId(request.plot))
        .ok_or_else(|| format!("plot {} does not exist", request.plot))?;
    if config.kind != LINE_PLOT_KIND {
        return Err("visualization is not a line plot".into());
    }
    let mut style = LinePlotStyle::load(config);
    if let Some(label) = &request.x_label {
        style.x_label = (!label.trim().is_empty()).then(|| label.clone());
    }
    if let Some(label) = &request.y_label {
        style.y_label = (!label.trim().is_empty()).then(|| label.clone());
    }
    if let Some(range) = &request.y_reference_range {
        style.y_reference_range = if range.is_empty() {
            None
        } else {
            Some([range[0], range[1]])
        };
    }
    if let Some(width) = request.line_width {
        style.line_width = width;
    }
    if let Some(color) = request.series_color.as_deref() {
        style.series_color = match color {
            [r, g, b] => Some([*r, *g, *b]),
            _ => None,
        };
    }
    if let Some(size) = request.text_size {
        style.text_size = (size > 0.0).then_some(size);
    }
    if let Some(controls) = request.show_controls {
        style.show_controls = controls;
    }
    if let Some(hover) = request.show_hover_values {
        style.show_hover_values = hover;
    }
    if let Some(legend) = request.show_legend {
        style.show_legend = legend;
    }
    if let Some(grid) = request.simplified_grid {
        style.simplified_grid = grid;
    }
    if let Some(latest) = request.show_latest_value {
        style.show_latest_value = latest;
    }
    if let Some(inside) = request.compact_layout {
        style.compact_layout = inside;
    }
    if let Some(caption) = &request.caption {
        style.caption = (!caption.trim().is_empty()).then(|| caption.clone());
    }
    if let Some(follow) = request.follow_latest {
        style.follow_latest = follow;
    }
    if let Some(window) = request.time_window_s {
        style.time_window_s = (window > 0.0).then_some(window);
        style.time_window_revision = style.time_window_revision.wrapping_add(1);
    }
    style.save(config);
    Ok(())
}

#[on_command(ConfigureLinePlot)]
fn on_configure_line_plot(
    trigger: On<ConfigureLinePlot>,
    mut registry: ResMut<VisualizationRegistry>,
) -> Result<Ack, String> {
    configure_line_plot(&mut registry, trigger.event())?;
    Ok(Ack::new(OpId::new()))
}

/// Edit an existing live signal binding's presentation. Source identity and
/// samples stay unchanged; the ordinary binding persists the label and colour.
#[Command(default)]
pub struct ConfigurePlotBinding {
    pub plot: u64,
    pub signal: String,
    /// Empty restores the shared telemetry label.
    pub label: Option<String>,
    /// Empty restores the theme; otherwise exactly three RGB bytes.
    pub color: Option<Vec<u8>>,
}

fn configure_plot_binding(
    registry: &mut VisualizationRegistry,
    request: &ConfigurePlotBinding,
) -> Result<(), String> {
    let color = match request.color.as_deref() {
        None => None,
        Some([]) => Some(None),
        Some([r, g, b]) => Some(Some(egui::Color32::from_rgb(*r, *g, *b))),
        Some(_) => return Err("binding colour must be empty or three RGB bytes".into()),
    };
    let config = registry
        .get_mut(lunco_viz_core::VizId(request.plot))
        .ok_or_else(|| format!("plot {} does not exist", request.plot))?;
    if config.kind != LINE_PLOT_KIND {
        return Err("visualization is not a line plot".into());
    }
    if !config
        .inputs
        .iter()
        .any(|b| b.source.path == request.signal)
    {
        return Err(format!("plot has no live binding for {}", request.signal));
    }
    for binding in config
        .inputs
        .iter_mut()
        .filter(|b| b.source.path == request.signal)
    {
        if let Some(label) = &request.label {
            binding.label = (!label.trim().is_empty()).then(|| label.clone());
        }
        if let Some(color) = color {
            binding.color = color;
        }
    }
    Ok(())
}

#[on_command(ConfigurePlotBinding)]
fn on_configure_plot_binding(
    trigger: On<ConfigurePlotBinding>,
    mut registry: ResMut<VisualizationRegistry>,
) -> Result<Ack, String> {
    configure_plot_binding(&mut registry, trigger.event())?;
    Ok(Ack::new(OpId::new()))
}

register_commands!(on_configure_line_plot, on_configure_plot_binding);

// ── Per-plot toolbar + editing ──────────────────────────────────────

/// The mutation the toolbar asked for, applied in a second pass so
/// we don't hold `&VisualizationConfig` while mutating the registry.
#[derive(Clone)]
enum Edit {
    SetX(Option<SignalRef>),
    AddY(SignalRef),
    RemoveY(SignalRef),
    SetLogY(bool),
    SetLineWidth(f32),
}

fn render_toolbar(
    ctx: &mut Panel2DCtx,
    config: &VisualizationConfig,
    actions: impl FnOnce(&mut egui::Ui, &mut PanelCtx),
) -> Option<Edit> {
    let style = LinePlotStyle::load_cached(ctx.ui.ctx(), config);

    let mut edit: Option<Edit> = None;
    let mut removed: Option<SignalRef> = None;

    // One control row: X picker, Y chips, add/log controls, and host actions.
    // Y bindings use a horizontal scroll surface so a large telemetry set
    // cannot grow the header vertically. Keeping every control in this row
    // preserves graph height and gives each plot one canonical toolbar.
    ctx.ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        // X picker.
        ui.label(
            egui::RichText::new("X:").text_style(lunco_theme::TypographyRole::Label.text_style()),
        );
        let x_current = style
            .x_signal
            .as_ref()
            .map(|s| component_parameter_label(ctx.wb, s))
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
                if let Some(registry) = ctx.wb.resource::<SignalRegistry>() {
                    let catalog = toolbar_signal_catalog(ui.ctx(), registry);
                    for signal in &catalog.signals {
                        let selected = style.x_signal.as_ref() == Some(&signal.source);
                        let label = component_parameter_label_with_meta(
                            ctx.wb,
                            &signal.source,
                            signal.meta.as_ref(),
                        );
                        if ui.selectable_label(selected, label).clicked() && !selected {
                            edit = Some(Edit::SetX(Some(signal.source.clone())));
                        }
                    }
                }
            });
        ui.separator();
        ui.label(
            egui::RichText::new("Y:").text_style(lunco_theme::TypographyRole::Label.text_style()),
        );
        // Reserve a compact action cluster on the right, and give the Y
        // binding strip every remaining pixel. This keeps long canonical
        // names readable without letting the action buttons drift into the
        // middle of the toolbar.
        let y_width = (ui.available_width() - 400.0).max(160.0);
        egui::ScrollArea::horizontal()
            .id_salt(("lp_y_bindings", config.id.raw()))
            .max_width(y_width)
            .max_height(24.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for b in config.inputs.iter().filter(|b| b.role == ROLE_Y.role) {
                        let meta = ctx
                            .wb
                            .resource::<SignalRegistry>()
                            .and_then(|registry| registry.meta(&b.source));
                        let chip = ui
                            .small_button(format!("{} x", binding_label(ctx.wb, b, meta)))
                            .on_hover_text("Remove from this plot");
                        let chip = if let Some(hint) = signal_hint(meta) {
                            chip.on_hover_text(hint)
                        } else {
                            chip
                        };
                        if chip.clicked() {
                            removed = Some(b.source.clone());
                        }
                    }
                });
            });

        // Add/log/actions stay in a right-aligned cluster. Add remains
        // outside the scrolling list so it is always reachable.
        ui.add_space((ui.available_width() - 400.0).max(0.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            actions(ui, ctx.wb);
            ui.separator();
            let mut line_width = style.line_width;
            if ui
                .add(
                    egui::DragValue::new(&mut line_width)
                        .range(0.5..=8.0)
                        .speed(0.1)
                        .fixed_decimals(1)
                        .prefix("Line ")
                        .suffix(" px"),
                )
                .on_hover_text("Curve thickness; does not change UI scale or signal samples")
                .changed()
            {
                edit = Some(Edit::SetLineWidth(line_width));
            }
            ui.separator();
            let mut log_y = style.log_y;
            if ui
                .toggle_value(&mut log_y, "log Y")
                .on_hover_text("Plot the Y axis on a log10 scale (drops values <= 0)")
                .changed()
            {
                edit = Some(Edit::SetLogY(log_y));
            }
            ui.separator();
            egui::ComboBox::from_id_salt(("lp_add", config.id.raw()))
                .selected_text("Add")
                .width(120.0)
                .show_ui(ui, |ui| {
                    let Some(registry) = ctx.wb.resource::<SignalRegistry>() else {
                        ui.label("SignalRegistry not installed.");
                        return;
                    };
                    let catalog = toolbar_signal_catalog(ui.ctx(), registry);
                    let mut has_addable = false;
                    for signal in &catalog.signals {
                        if config.inputs.iter().any(|binding| {
                            binding.role == ROLE_Y.role && binding.source == signal.source
                        }) {
                            continue;
                        }
                        has_addable = true;
                        let label = component_parameter_label_with_meta(
                            ctx.wb,
                            &signal.source,
                            signal.meta.as_ref(),
                        );
                        let button = ui.button(label);
                        let button = if let Some(hint) = &signal.hint {
                            button.on_hover_text(hint)
                        } else {
                            button
                        };
                        if button.clicked() {
                            edit = Some(Edit::AddY(signal.source.clone()));
                        }
                    }
                    if !has_addable {
                        ui.label(if catalog.signals.is_empty() {
                            "No scalar signals are available yet."
                        } else {
                            "All scalar signals are already plotted."
                        });
                    }
                });
        });
    });
    if let Some(r) = removed {
        edit = Some(Edit::RemoveY(r));
    }
    edit
}

fn apply_edit(registry: &mut VisualizationRegistry, viz: lunco_viz_core::VizId, edit: Edit) {
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
                cfg.inputs.push(SignalBinding::live(sig, ROLE_Y.role));
            }
        }
        Edit::RemoveY(sig) => {
            cfg.inputs.retain(|b| b.source != sig);
        }
        Edit::SetLogY(on) => {
            let mut style = LinePlotStyle::load(cfg);
            style.log_y = on;
            style.save(cfg);
        }
        Edit::SetLineWidth(width) => {
            let mut style = LinePlotStyle::load(cfg);
            style.line_width = width;
            style.save(cfg);
        }
    }
}

pub(crate) fn on_line_plot_edit_requested(
    trigger: On<LinePlotEditRequested>,
    mut registry: ResMut<VisualizationRegistry>,
) {
    let request = trigger.event();
    apply_edit(&mut registry, request.viz, request.edit.clone());
}

pub(crate) fn on_line_plot_fit_requested(
    trigger: On<LinePlotFitRequested>,
    mut requests: ResMut<VizFitRequests>,
) {
    requests.take(trigger.viz);
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
    #[test]
    fn binding_presentation_is_persistent_and_rejects_invalid_edits_atomically() {
        let mut registry = VisualizationRegistry::default();
        let id = lunco_viz_core::VizId(42);
        let signal = SignalRef::new(Entity::PLACEHOLDER, "velocity");
        registry.insert(VisualizationConfig {
            id,
            title: "Measured motion".into(),
            kind: LINE_PLOT_KIND,
            view: crate::view::ViewTarget::Panel2D,
            inputs: vec![SignalBinding::live(signal.clone(), "y")],
            style: serde_json::Value::Null,
        });
        let request = ConfigurePlotBinding {
            plot: id.0,
            signal: "velocity".into(),
            label: Some("Speed [m/s]".into()),
            color: Some(vec![120, 180, 255]),
        };
        configure_plot_binding(&mut registry, &request).unwrap();
        let serialized = serde_json::to_value(registry.get(id).unwrap()).unwrap();
        let restored: VisualizationConfig = serde_json::from_value(serialized.clone()).unwrap();
        assert_eq!(restored.inputs[0].source, signal);
        assert_eq!(restored.inputs[0].label.as_deref(), Some("Speed [m/s]"));
        assert_eq!(
            restored.inputs[0].color,
            Some(egui::Color32::from_rgb(120, 180, 255))
        );
        for invalid in [
            ConfigurePlotBinding {
                signal: "missing".into(),
                ..request.clone()
            },
            ConfigurePlotBinding {
                color: Some(vec![255]),
                ..request.clone()
            },
        ] {
            assert!(configure_plot_binding(&mut registry, &invalid).is_err());
            assert_eq!(
                serde_json::to_value(registry.get(id).unwrap()).unwrap(),
                serialized
            );
        }
        configure_plot_binding(
            &mut registry,
            &ConfigurePlotBinding {
                label: Some(String::new()),
                color: Some(vec![]),
                ..request
            },
        )
        .unwrap();
        let restored = registry.get(id).unwrap();
        assert!(restored.inputs[0].color.is_none() && restored.inputs[0].label.is_none());
    }

    use super::*;

    fn s(time: f64, value: f64) -> ScalarSample {
        ScalarSample { time, value }
    }

    #[test]
    fn pair_by_time_emits_one_point_per_y_after_first_x() {
        let xs = vec![s(0.0, 10.0), s(1.0, 20.0), s(2.0, 30.0)];
        let ys = vec![
            s(0.0, 100.0),
            s(0.5, 110.0),
            s(1.0, 120.0),
            s(1.5, 130.0),
            s(2.0, 140.0),
        ];
        let got = pair_by_time(&xs, ys);
        // piecewise-constant X: y@0 pairs with x@0=10; y@0.5 still with x@0=10;
        // y@1 with x@1=20; y@1.5 still with x@1=20; y@2 with x@2=30.
        assert_eq!(
            got,
            vec![
                [10.0, 100.0],
                [10.0, 110.0],
                [20.0, 120.0],
                [20.0, 130.0],
                [30.0, 140.0]
            ]
        );
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
        use crate::viz::VisualizationConfig;
        use lunco_viz_core::VizId;
        let mut cfg = VisualizationConfig {
            id: VizId(42),
            title: "t".into(),
            kind: LINE_PLOT_KIND,
            view: crate::view::ViewTarget::Panel2D,
            inputs: vec![],
            style: serde_json::Value::Null,
        };
        let style = LinePlotStyle {
            x_signal: Some(SignalRef::new(bevy::prelude::Entity::PLACEHOLDER, "phi")),
            x_persisted_signal: None,
            x_label: Some("phi [rad]".into()),
            y_label: Some("w [rad/s]".into()),
            y_reference_range: Some([-10.0, 10.0]),
            log_y: true,
            line_width: 4.0,
            series_color: Some([120, 180, 255]),
            text_size: Some(24.0),
            show_controls: false,
            show_hover_values: false,
            show_legend: false,
            simplified_grid: true,
            compact_layout: true,
            show_latest_value: true,
            caption: Some("Two identical valve pulses".into()),
            follow_latest: true,
            time_window_s: Some(5.0),
            time_window_revision: 0,
        };
        style.save(&mut cfg);
        let roundtrip = LinePlotStyle::load(&cfg);
        assert_eq!(roundtrip.x_signal.map(|s| s.path), Some("phi".into()));
        assert_eq!(roundtrip.x_label.as_deref(), Some("phi [rad]"));
        assert_eq!(roundtrip.y_label.as_deref(), Some("w [rad/s]"));
        assert_eq!(roundtrip.y_reference_range, Some([-10.0, 10.0]));
        assert!(roundtrip.log_y);
        assert_eq!(roundtrip.line_width, 4.0);
        assert_eq!(roundtrip.text_size, Some(24.0));
        assert!(!roundtrip.show_controls);
        assert!(!roundtrip.show_hover_values);
        assert!(!roundtrip.show_legend);
        assert!(roundtrip.simplified_grid);
        assert!(roundtrip.compact_layout);
        assert!(roundtrip.show_latest_value);
        assert_eq!(
            roundtrip.caption.as_deref(),
            Some("Two identical valve pulses")
        );
        assert!(roundtrip.follow_latest);
        assert_eq!(roundtrip.time_window_s, Some(5.0));
    }

    #[test]
    fn presentation_style_edits_preserve_signal_bindings_and_validate_atomically() {
        use lunco_viz_core::VizId;
        let mut registry = VisualizationRegistry::default();
        registry.insert(VisualizationConfig {
            id: VizId(42),
            title: "Tank temperature".into(),
            kind: LINE_PLOT_KIND,
            view: crate::view::ViewTarget::Panel2D,
            inputs: vec![SignalBinding::live(
                SignalRef::new(Entity::PLACEHOLDER, "temperature"),
                "y",
            )],
            style: serde_json::json!({"log_y": true}),
        });
        assert!(LinePlotStyle::load(registry.get(VizId(42)).unwrap()).show_controls);
        configure_line_plot(
            &mut registry,
            &ConfigureLinePlot {
                plot: 42,
                x_label: Some("Time [s]".into()),
                y_label: Some("Temperature [K]".into()),
                y_reference_range: Some(vec![190.0, 310.0]),
                line_width: Some(4.0),
                series_color: Some(vec![255, 195, 105]),
                text_size: Some(24.0),
                show_controls: Some(false),
                show_hover_values: Some(false),
                show_legend: Some(false),
                simplified_grid: Some(true),
                compact_layout: Some(true),
                show_latest_value: Some(true),
                caption: Some("Two identical valve pulses".into()),
                follow_latest: Some(true),
                time_window_s: Some(5.0),
            },
        )
        .unwrap();
        let config = registry.get(VizId(42)).unwrap();
        let before = config.style.clone();
        assert_eq!(config.inputs[0].source.path.as_str(), "temperature");
        let style = LinePlotStyle::load(config);
        assert!(style.log_y);
        assert!(!style.show_controls);
        assert!(!style.show_hover_values);
        assert!(!style.show_legend);
        assert!(style.simplified_grid);
        assert!(style.compact_layout);
        assert!(style.show_latest_value);
        assert_eq!(style.caption.as_deref(), Some("Two identical valve pulses"));
        assert_eq!(style.text_size, Some(24.0));
        assert!(style.follow_latest);
        assert_eq!(style.time_window_s, Some(5.0));
        assert_eq!(style.y_label.as_deref(), Some("Temperature [K]"));
        assert_eq!(style.y_reference_range, Some([190.0, 310.0]));
        for range in [
            vec![0.0],
            vec![2.0, 1.0],
            vec![1.0, f64::NAN],
            vec![1.0, 2.0, 3.0],
        ] {
            assert!(
                configure_line_plot(
                    &mut registry,
                    &ConfigureLinePlot {
                        plot: 42,
                        y_label: Some("must not apply".into()),
                        y_reference_range: Some(range),
                        ..Default::default()
                    }
                )
                .is_err()
            );
            assert_eq!(registry.get(VizId(42)).unwrap().style, before);
        }
        for width in [f32::NAN, f32::INFINITY, 0.0, 9.0] {
            assert!(
                configure_line_plot(
                    &mut registry,
                    &ConfigureLinePlot {
                        plot: 42,
                        y_label: Some("must not apply".into()),
                        line_width: Some(width),
                        ..Default::default()
                    }
                )
                .is_err()
            );
            assert_eq!(registry.get(VizId(42)).unwrap().style, before);
        }
        for window in [f64::NAN, f64::INFINITY, -1.0] {
            assert!(
                configure_line_plot(
                    &mut registry,
                    &ConfigureLinePlot {
                        plot: 42,
                        y_label: Some("must not apply".into()),
                        time_window_s: Some(window),
                        ..Default::default()
                    }
                )
                .is_err()
            );
            assert_eq!(registry.get(VizId(42)).unwrap().style, before);
        }
        configure_line_plot(
            &mut registry,
            &ConfigureLinePlot {
                plot: 42,
                y_label: Some(" ".into()),
                y_reference_range: Some(vec![]),
                time_window_s: Some(0.0),
                ..Default::default()
            },
        )
        .unwrap();
        let style = LinePlotStyle::load(registry.get(VizId(42)).unwrap());
        assert!(style.y_label.is_none());
        assert!(style.y_reference_range.is_none());
        assert!(style.follow_latest);
        assert!(style.time_window_s.is_none());
        assert_eq!(style.line_width, 4.0);
        assert_eq!(style.x_label.as_deref(), Some("Time [s]"));
    }
}
