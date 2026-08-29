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
//! populated from the `SignalRegistry`. Without this toolbar, new
//! plots appeared frozen to users because the only signal-picker
//! (Telemetry's checkboxes) targeted the default plot exclusively.
//!
//! Style is stored in `VisualizationConfig.style` as
//! [`LinePlotStyle`] (serde JSON) so the choice survives save/reload.

use bevy::prelude::*;
use bevy_egui::egui;
use egui_plot::{Corner, Legend, Line, Plot, PlotPoints};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::registry::{VisualizationRegistry, VizFitRequests};
use crate::signal::{
    PersistedSignalRef, ScalarSample, SignalMeta, SignalRef, SignalRegistry, SignalType,
};
use crate::view::{Panel2DCtx, ViewKind};
use crate::viz::{RoleSpec, SignalBinding, Visualization, VisualizationConfig, VizKindId};
use lunco_core::GlobalEntityId;
use lunco_workbench::PanelCtx;

/// A line-plot toolbar edit requested by the panel's read-only paint pass.
#[derive(Event)]
pub(crate) struct LinePlotEditRequested {
    pub(crate) viz: crate::viz::VizId,
    edit: Edit,
}

/// A one-shot request to reset a plot's egui bounds on its next render.
#[derive(Event, Clone, Copy)]
pub(crate) struct LinePlotFitRequested {
    pub(crate) viz: crate::viz::VizId,
}

/// LinePlot-specific options stashed in
/// [`VisualizationConfig::style`]. Serialised as JSON for on-disk
/// round-trip. All fields optional — missing fields keep default
/// behaviour (X = time, auto-labeled axes).
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
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
    /// Plot the Y axis on a log10 scale. Values ≤ 0 are dropped (gaps,
    /// not clamped). Persists with the plot like the other style.
    #[serde(default)]
    pub log_y: bool,
}

impl LinePlotStyle {
    fn load(config: &VisualizationConfig) -> Self {
        serde_json::from_value(config.style.clone()).unwrap_or_default()
    }
    /// Render-path loader: parse the style JSON once and cache the
    /// result in egui context data, keyed by the blob itself. The blob
    /// only changes on a user edit, so steady-state frames cost a
    /// `Value` equality check instead of two serde deserializations
    /// per plot per frame (toolbar + body both call this).
    fn load_cached(ctx: &egui::Context, config: &VisualizationConfig) -> Self {
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
    fn save(&self, config: &mut VisualizationConfig) {
        config.style = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
    }
}

/// Operator-facing identity for an entity-scoped signal.  `SignalRef::path`
/// alone is intentionally not unique — four wheels can all publish
/// `axle_torque` — so every graph surface shows the owning USD prim as well.
/// The full path remains available in telemetry tooltips and persistence keeps
/// the entity identity; this is solely the concise plot presentation.
fn component_parameter_label(wb: &PanelCtx, signal: &SignalRef) -> String {
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
    let meta = wb
        .resource::<SignalRegistry>()
        .and_then(|registry| registry.meta(signal));
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
        .unwrap_or_else(|| component_parameter_label(wb, &binding.source));
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
        first_t: h.samples.front().map_or(0, |s| s.time.to_bits()),
        last_t: h.samples.back().map_or(0, |s| s.time.to_bits()),
    }
}

/// Return an owned time-series presentation buffer that is rebuilt only when
/// the source history changes. Plot hosts that combine this visualization's
/// live curves with another plot surface use this owner instead of copying a
/// ring buffer during every panel paint.
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
    type Cached = std::sync::Arc<(HistFingerprint, std::sync::Arc<Vec<[f64; 2]>>)>;
    if let Some(cached) = ctx.data(|data| data.get_temp::<Cached>(cache_id)) {
        if cached.0 == key {
            return Some(cached.1.clone());
        }
    }
    let points: std::sync::Arc<Vec<[f64; 2]>> =
        std::sync::Arc::new(history.iter().map(|s| [s.time, s.value]).collect());
    ctx.data_mut(|data| {
        data.insert_temp(cache_id, std::sync::Arc::new((key, points.clone())));
    });
    Some(points)
}

/// Everything a cached tessellation depends on. Stored next to the
/// points; a mismatch on any component forces a rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SeriesKey {
    y: HistFingerprint,
    /// Fingerprint of the X signal's history in phase-space mode.
    x: Option<HistFingerprint>,
    log_y: bool,
    /// Plot pixel width bucket — decimation depth depends on it.
    px_w: u32,
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
        let edit = render_toolbar(ctx, config, actions);
        if let Some(edit) = edit {
            // Queue the registry mutation for after the egui pass —
            // render holds only read access to the world.
            let id = config.id;
            ctx.wb.trigger(LinePlotEditRequested { viz: id, edit });
            // Don't render the body this frame — the config just
            // changed. Next frame picks up the new bindings.
            return;
        }

        let style = LinePlotStyle::load_cached(ctx.ui.ctx(), config);
        let registry = match ctx.wb.resource::<SignalRegistry>() {
            Some(r) => r,
            None => {
                ctx.ui.label("SignalRegistry not installed.");
                return;
            }
        };

        // Collect `y`-role bindings. Filter hidden + missing-signal
        // bindings here so the legend shows the same set as the plot.
        let y_bindings: Vec<&SignalBinding> = config
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
                    .size(10.0)
                    .color(muted),
                );
            });
            return;
        }

        // A scene replacement removes the previous entity-scoped histories
        // before the replacement source has emitted. Do not render that as a
        // blank chart: tell the participant exactly what is happening and keep
        // the binding alive for the lifecycle reconciler to restore.
        let unavailable: Vec<String> = y_bindings
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
        let remaining = ctx.ui.available_size_before_wrap();

        // Resolve the X-axis source once. For classic time-series we
        // just use each Y sample's own `time`. For phase-space mode
        // we pull the X signal's history and pair by time below.
        // Fingerprint travels into each series' cache key so an X-side
        // change also dirties the pairing.
        let x_fp: Option<HistFingerprint> = style
            .x_signal
            .as_ref()
            .and_then(|xs| registry.scalar_history(xs))
            .filter(|h| !h.is_empty())
            .map(hist_fingerprint);
        // Lazily materialised on the first cache miss — an idle frame
        // (all series clean) never copies the X history at all. The
        // outer `Option` is "not fetched yet", the inner one is the
        // classic "no usable X signal → time on X" fallback.
        let mut x_samples: Option<Option<Vec<ScalarSample>>> = None;

        // Build the egui_plot `Line`s up-front so we can release the
        // registry borrow before calling `plot.show()` (which wants a
        // long-lived borrow on `ctx.ui`).
        //
        // Tessellation is dirty-checked: the (history fingerprint,
        // x fingerprint, log_y, pixel width) key is compared against a
        // per-binding cache in egui context data; only a moved key
        // re-copies the history, re-logs, and re-decimates. Steady
        // frames clone the cached (pixel-bounded) point vec only —
        // that owned copy is the one egui_plot's `PlotPoints` demands.
        let lines: Vec<Line> = y_bindings
            .iter()
            .filter_map(|b| {
                let hist = registry.scalar_history(&b.source)?;
                if hist.is_empty() {
                    return None;
                }
                let key = SeriesKey {
                    y: hist_fingerprint(hist),
                    x: x_fp,
                    log_y: style.log_y,
                    px_w: remaining.x.max(1.0) as u32,
                };
                let cache_id = egui::Id::new(("line_plot_series", config.id.raw())).with(&b.source);
                let cached: Option<std::sync::Arc<(SeriesKey, Vec<[f64; 2]>)>> =
                    ctx.ui.ctx().data(|d| d.get_temp(cache_id));
                let series = match cached {
                    Some(c) if c.0 == key => c,
                    _ => {
                        let xs_resolved = x_samples.get_or_insert_with(|| {
                            style.x_signal.as_ref().and_then(|xs| {
                                registry
                                    .scalar_history(xs)
                                    .filter(|h| !h.is_empty())
                                    .map(|h| h.iter().copied().collect())
                            })
                        });
                        let time_on_x = xs_resolved.is_none();
                        let mut pts: Vec<[f64; 2]> = match xs_resolved {
                            None => {
                                // Classic time on X. Each sample's own
                                // `time` is its X coordinate.
                                hist.iter().map(|s| [s.time, s.value]).collect()
                            }
                            Some(xs) => pair_by_time(xs, hist.iter().copied()),
                        };
                        if style.log_y {
                            pts = crate::plot_fmt::log_y_points(&pts);
                        }
                        // Decimate to pixel width — min-max buckets so
                        // spikes survive. Time-series only: a phase-
                        // space trajectory revisits X, which breaks
                        // the column bucketing.
                        if time_on_x {
                            if let Some(dec) = crate::plot_fmt::decimate_min_max(&pts, remaining.x)
                            {
                                pts = dec;
                            }
                        }
                        let fresh = std::sync::Arc::new((key, pts));
                        ctx.ui
                            .ctx()
                            .data_mut(|d| d.insert_temp(cache_id, fresh.clone()));
                        fresh
                    }
                };
                if series.1.is_empty() {
                    return None;
                }
                // Entity identity is part of the visible label: four physical
                // wheels may all expose `axle_torque` at once.
                let meta = registry.meta(&b.source);
                let label = binding_label(ctx.wb, b, meta);
                let color = b
                    .color
                    .unwrap_or_else(|| crate::signal::color_for_signal(&theme, &b.source.path));
                Some(Line::new(label, PlotPoints::new(series.1.clone())).color(color))
            })
            .collect();

        if lines.is_empty() {
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
                    .size(10.0)
                    .color(muted),
                );
                ui.label(
                    egui::RichText::new(
                        "The plot will reconnect automatically when this scene publishes it.",
                    )
                    .size(10.0)
                    .color(muted),
                );
            });
            return;
        }

        if !unavailable.is_empty() {
            ctx.ui.label(
                egui::RichText::new(format!("Waiting for: {}", unavailable.join(", ")))
                    .size(10.0)
                    .color(
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
            .width(remaining.x)
            .height(remaining.y)
            .auto_bounds(bevy_egui::egui::emath::Vec2b::new(true, true))
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
        if let Some(label) = x_label {
            plot = plot.x_axis_label(label);
        }
        if let Some(label) = y_label {
            plot = plot.y_axis_label(label);
        }
        if y_bindings.len() > 1 {
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

        plot.show(ctx.ui, |plot_ui| {
            for line in lines {
                plot_ui.line(line);
            }
        });
    }
}

// ── Per-plot toolbar + editing ──────────────────────────────────────

/// The mutation the toolbar asked for, applied in a second pass so
/// we don't hold `&VisualizationConfig` while mutating the registry.
#[derive(Clone)]
enum Edit {
    SetX(Option<SignalRef>),
    AddY(SignalRef),
    RemoveY(SignalRef),
    SetLogY(bool),
}

fn render_toolbar(
    ctx: &mut Panel2DCtx,
    config: &VisualizationConfig,
    actions: impl FnOnce(&mut egui::Ui, &mut PanelCtx),
) -> Option<Edit> {
    // Snapshot available signals + current style so we can render
    // without holding a long-lived registry borrow.
    let registry = ctx.wb.resource::<SignalRegistry>();
    let (available, current_y_paths): (Vec<SignalRef>, std::collections::HashSet<SignalRef>) = {
        let available: Vec<SignalRef> = registry
            .map(|r| {
                r.iter_signals()
                    .filter(|&(_, t)| t == SignalType::Scalar)
                    .map(|(s, _)| s.clone())
                    .collect()
            })
            .unwrap_or_default();
        let current: std::collections::HashSet<SignalRef> = config
            .inputs
            .iter()
            .filter(|b| b.role == ROLE_Y.role)
            .map(|b| b.source.clone())
            .collect();
        (available, current)
    };
    let style = LinePlotStyle::load_cached(ctx.ui.ctx(), config);
    let muted = ctx
        .wb
        .resource::<lunco_theme::Theme>()
        .map(|t| t.tokens.text_subdued)
        .unwrap_or(egui::Color32::DARK_GRAY);

    // Resolve registry metadata for a signal at most once per toolbar render.
    // Keep owned metadata here so the row can also issue a mutable host action
    // without holding an immutable borrow of the SignalRegistry through the
    // egui closure. The previous version asked the registry for `meta(&source)`
    // twice per Y chip and once more per addable.
    let metadata: HashMap<SignalRef, SignalMeta> = available
        .iter()
        .filter_map(|sig| {
            registry
                .and_then(|r| r.meta(sig))
                .cloned()
                .map(|meta| (sig.clone(), meta))
        })
        .collect();
    let meta_of = |sig: &SignalRef| metadata.get(sig);

    let mut edit: Option<Edit> = None;
    let mut removed: Option<SignalRef> = None;

    // One control row: X picker, Y chips, add/log controls, and host actions.
    // Y bindings use a horizontal scroll surface so a large telemetry set
    // cannot grow the header vertically. Keeping every control in this row
    // preserves graph height and gives each plot one canonical toolbar.
    ctx.ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        // X picker.
        ui.label(egui::RichText::new("X:").size(11.0));
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
                for sig in &available {
                    let selected = style.x_signal.as_ref() == Some(sig);
                    if ui
                        .selectable_label(selected, component_parameter_label(ctx.wb, sig))
                        .clicked()
                        && !selected
                    {
                        edit = Some(Edit::SetX(Some(sig.clone())));
                    }
                }
            });
        ui.separator();
        ui.label(egui::RichText::new("Y:").size(11.0));
        // Reserve a compact action cluster on the right, and give the Y
        // binding strip every remaining pixel. This keeps long canonical
        // names readable without letting the action buttons drift into the
        // middle of the toolbar.
        let y_width = (ui.available_width() - 300.0).max(160.0);
        egui::ScrollArea::horizontal()
            .id_salt(("lp_y_bindings", config.id.raw()))
            .max_width(y_width)
            .max_height(24.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for b in config.inputs.iter().filter(|b| b.role == ROLE_Y.role) {
                        let meta = meta_of(&b.source);
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
        ui.add_space((ui.available_width() - 300.0).max(0.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            actions(ui, ctx.wb);
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
            let addables: Vec<&SignalRef> = available
                .iter()
                .filter(|s| !current_y_paths.contains(s))
                .collect();
            if !addables.is_empty() {
                egui::ComboBox::from_id_salt(("lp_add", config.id.raw()))
                    .selected_text("Add")
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        for sig in addables {
                            let meta = meta_of(sig);
                            let button = ui.button(component_parameter_label(ctx.wb, sig));
                            let button = if let Some(hint) = signal_hint(meta) {
                                button.on_hover_text(hint)
                            } else {
                                button
                            };
                            if button.clicked() {
                                edit = Some(Edit::AddY(sig.clone()));
                            }
                        }
                    });
            } else if current_y_paths.is_empty() {
                ui.label(
                    egui::RichText::new("no signals yet")
                        .color(muted)
                        .size(10.0),
                );
            }
        });
    });
    if let Some(r) = removed {
        edit = Some(Edit::RemoveY(r));
    }
    edit
}

fn apply_edit(registry: &mut VisualizationRegistry, viz: crate::viz::VizId, edit: Edit) {
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
    fn cached_scalar_history_points_reuses_unchanged_history() {
        let ctx = egui::Context::default();
        let signal = SignalRef::new(Entity::from_raw_u32(1).unwrap(), "speed");
        let mut registry = SignalRegistry::with_default_capacity(8);
        registry.push_scalar(signal.clone(), 0.0, 1.0);

        let first = cached_scalar_history_points(&ctx, &registry, &signal).unwrap();
        let second = cached_scalar_history_points(&ctx, &registry, &signal).unwrap();
        assert!(std::sync::Arc::ptr_eq(&first, &second));

        registry.push_scalar(signal.clone(), 1.0, 2.0);
        let changed = cached_scalar_history_points(&ctx, &registry, &signal).unwrap();
        assert!(!std::sync::Arc::ptr_eq(&first, &changed));
        assert_eq!(changed.as_slice(), &[[0.0, 1.0], [1.0, 2.0]]);
    }

    #[test]
    fn line_plot_style_round_trips_through_config_json() {
        use crate::viz::{VisualizationConfig, VizId};
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
            log_y: true,
        };
        style.save(&mut cfg);
        let roundtrip = LinePlotStyle::load(&cfg);
        assert_eq!(roundtrip.x_signal.map(|s| s.path), Some("phi".into()));
        assert_eq!(roundtrip.x_label.as_deref(), Some("phi [rad]"));
        assert_eq!(roundtrip.y_label.as_deref(), Some("w [rad/s]"));
        assert!(roundtrip.log_y);
    }
}
