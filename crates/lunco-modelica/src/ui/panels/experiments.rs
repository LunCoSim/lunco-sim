//! Experiments panel — list of Fast Run experiments per twin.
//!
//! Status / spec: `docs/architecture/25-experiments.md`. v1 minimal:
//! - List each experiment in the registry (currently single "default" twin).
//! - Show name, bounds, status, duration, error.
//! - Plot-visibility checkbox (consumed by Graphs panel in Step 7).
//! - Cancel button on Running rows.
//! - Delete on terminal rows (context menu / button).
//!
//! Color picker, inline rename, click-to-load-draft are TODOs left
//! for the v1 polish pass.

use std::collections::BTreeMap;

use bevy::prelude::*;
use bevy_egui::egui;
use egui_plot::{Legend, Line, LineStyle, Plot, PlotPoints, VLine};
use lunco_experiments::{
    ExperimentId, ExperimentRegistry, RunStatus,
};
use lunco_viz::viz::VizId;
use lunco_workbench::{Panel, PanelId, PanelSlot};

pub const EXPERIMENTS_PANEL_ID: PanelId = PanelId("modelica_experiments");

/// UI-only state attached to the experiments panel that has no
/// natural home on a per-plot basis: the variable-picker filter,
/// inline-rename buffer, and the Telemetry "Plot in" router target.
///
/// Per-plot experiment visibility lives on [`PlotPanelState`] —
/// each plot tab toggles its own checked runs, so switching tabs
/// shows a different set of curves (OMEdit / Dymola-style "Plot
/// Window" semantics).
#[derive(Resource, Default, Debug)]
pub struct ExperimentVisibility {
    /// Free-text filter for the variable picker. Case-insensitive
    /// substring match against the dotted variable path.
    pub var_filter: String,
    /// Inline-rename state. `Some((id, draft_text))` → row `id`
    /// renders a `TextEdit` instead of a `Label`; `None` → all rows
    /// show their name as a plain label. Committed on Enter or
    /// focus-loss.
    pub editing_name: Option<(ExperimentId, String)>,
    /// Telemetry's "Plot in" router target. `None` ⇒ route to the
    /// active plot (`ActivePlot::or_default()`); `Some(viz)` pins
    /// Telemetry checkboxes to a specific plot tab regardless of
    /// which one is focused. Mirrors Dymola's "current plot window"
    /// pin.
    pub target_plot: Option<VizId>,
}

/// Per-plot-panel state — picked variables, scrub cursor, and the
/// set of experiments visible *in this plot*. Keyed by `VizId` so
/// each plot tab maintains independent picks and run-visibility
/// (OMEdit / Dymola treat each Plot Window as an independent view
/// over the same result store).
///
/// `last_twin` lets the plot drop stale `picked_vars` /
/// `visible_experiments` when the resolved document switches
/// (different doc → different `TwinId` → different variable
/// namespace + experiment ids). Without this, ids ticked while
/// viewing doc A would linger as zombies after switching to doc B.
#[derive(Default, Debug, Clone)]
pub struct PlotPanelState {
    pub picked_vars: std::collections::BTreeSet<String>,
    pub scrub_time: Option<f64>,
    pub visible_experiments: std::collections::HashSet<ExperimentId>,
    pub last_twin: Option<lunco_experiments::TwinId>,
    /// True once the plot has auto-promoted the latest run for this
    /// twin (mark-visible + pick top dynamic vars). Prevents the
    /// auto-show from fighting the user after they explicitly empty
    /// the plot. Reset on twin switch by `sync_twin`'s restore path
    /// (a fresh state for a new twin starts at `false`).
    pub auto_show_attempted: bool,
}

#[derive(Resource, Default, Debug)]
pub struct PlotPanelStates {
    pub by_viz: std::collections::HashMap<VizId, PlotPanelState>,
    /// Archived per-(viz, twin) state. When a plot's resolved twin
    /// changes (user switches to a tab backed by a different model),
    /// the live entry's prior state is stashed here keyed by the
    /// previous twin; returning to that twin restores the picks /
    /// run-visibility / scrub. Without this archive, switching tabs
    /// would discard the prior plot's curve selections entirely.
    archived: std::collections::HashMap<
        (VizId, lunco_experiments::TwinId),
        PlotPanelState,
    >,
}

impl PlotPanelStates {
    pub fn get(&self, viz: VizId) -> Option<&PlotPanelState> {
        self.by_viz.get(&viz)
    }
    pub fn entry(&mut self, viz: VizId) -> &mut PlotPanelState {
        self.by_viz.entry(viz).or_default()
    }
    pub fn picked(&self, viz: VizId) -> std::collections::BTreeSet<String> {
        self.by_viz
            .get(&viz)
            .map(|s| s.picked_vars.clone())
            .unwrap_or_default()
    }
    pub fn scrub(&self, viz: VizId) -> Option<f64> {
        self.by_viz.get(&viz).and_then(|s| s.scrub_time)
    }
    pub fn toggle_var(&mut self, viz: VizId, var: String) {
        let s = self.entry(viz);
        if !s.picked_vars.insert(var.clone()) {
            s.picked_vars.remove(&var);
        }
    }
    pub fn set_var(&mut self, viz: VizId, var: String, on: bool) {
        let s = self.entry(viz);
        if on {
            s.picked_vars.insert(var);
        } else {
            s.picked_vars.remove(&var);
        }
    }
    pub fn set_scrub(&mut self, viz: VizId, t: Option<f64>) {
        self.entry(viz).scrub_time = t;
    }
    pub fn visible(&self, viz: VizId) -> std::collections::HashSet<ExperimentId> {
        self.by_viz
            .get(&viz)
            .map(|s| s.visible_experiments.clone())
            .unwrap_or_default()
    }
    pub fn is_visible(&self, viz: VizId, id: ExperimentId) -> bool {
        self.by_viz
            .get(&viz)
            .is_some_and(|s| s.visible_experiments.contains(&id))
    }
    pub fn toggle_visible(&mut self, viz: VizId, id: ExperimentId) {
        let s = self.entry(viz);
        if !s.visible_experiments.insert(id) {
            s.visible_experiments.remove(&id);
        }
    }
    pub fn set_visible(&mut self, viz: VizId, id: ExperimentId, on: bool) {
        let s = self.entry(viz);
        if on {
            s.visible_experiments.insert(id);
        } else {
            s.visible_experiments.remove(&id);
        }
    }
    /// Remove this experiment id from every plot's visibility set.
    /// Called when a run is deleted from the registry so stale ids
    /// don't linger.
    pub fn forget_experiment(&mut self, id: ExperimentId) {
        for s in self.by_viz.values_mut() {
            s.visible_experiments.remove(&id);
        }
        for s in self.archived.values_mut() {
            s.visible_experiments.remove(&id);
        }
    }

    /// Swap the live entry for `viz` to match `twin`, archiving any
    /// non-empty state from the previous twin and restoring a prior
    /// stash for `twin` if one exists. Idempotent when the twin is
    /// already current. Called at the top of `render_experiments_plot`
    /// each frame.
    pub fn sync_twin(&mut self, viz: VizId, twin: &lunco_experiments::TwinId) {
        let needs_swap = match self.by_viz.get(&viz) {
            Some(s) => s.last_twin.as_ref() != Some(twin),
            None => true,
        };
        if !needs_swap {
            return;
        }
        if let Some(prev) = self.by_viz.remove(&viz) {
            if let Some(prev_twin) = prev.last_twin.clone() {
                let worth_keeping = !prev.picked_vars.is_empty()
                    || !prev.visible_experiments.is_empty()
                    || prev.scrub_time.is_some();
                if worth_keeping {
                    self.archived.insert((viz, prev_twin), prev);
                }
            }
        }
        let mut restored = self
            .archived
            .remove(&(viz, twin.clone()))
            .unwrap_or_default();
        restored.last_twin = Some(twin.clone());
        self.by_viz.insert(viz, restored);
    }
}

/// Most-recently-rendered plot panel. Used by canvas overlay /
/// telemetry / runner so global readers can pick a sensible default
/// plot when they need per-plot state. Updated on every plot render.
#[derive(Resource, Default, Debug, Copy, Clone)]
pub struct ActivePlot(pub Option<VizId>);

impl ActivePlot {
    pub fn or_default(self) -> VizId {
        self.0.unwrap_or(crate::ui::viz::DEFAULT_MODELICA_GRAPH)
    }
}

pub struct ExperimentsPanel;

impl Panel for ExperimentsPanel {
    fn id(&self) -> PanelId {
        EXPERIMENTS_PANEL_ID
    }

    fn title(&self) -> String {
        "⚗ Experiments".into()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Bottom
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Pin header — follow active tab, or 📌 to lock the
        // experiments view to a specific model.
        crate::ui::doc_pin::render_pin_header(
            ui,
            world,
            crate::ui::doc_pin::PinKind::Experiments,
        );
        // Scope this panel to the doc-pin-resolved document. Each
        // open doc has its own run history (`twin_id_for_doc`), so
        // switching tabs flips the list automatically.
        let Some(doc_id) = crate::ui::doc_pin::resolved_experiments_doc(world)
        else {
            ui.label("No active document.");
            return;
        };
        let twin = crate::ui::doc_pin::twin_id_for_doc(doc_id);
        // Semantic colours from the theme. ThemePlugin is mandatory
        // (installed by WorkbenchPlugin), so this resource is always
        // present.
        let (col_success, col_warning, col_error, col_subdued) = {
            let t = world.resource::<lunco_theme::Theme>();
            (t.tokens.success, t.tokens.warning, t.tokens.error, t.tokens.text_subdued)
        };

        // One outer ScrollArea wraps Setup + Parameter overrides +
        // experiments table + empty-state copy so the user can reach
        // every section even when the bottom dock is short.
        egui::ScrollArea::vertical()
            .id_salt("experiments_panel_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
        self.render_setup_section(ui, world);
        ui.separator();
        self.render_override_editor(ui, world);
        ui.separator();

        // Snapshot for rendering — avoids holding the registry borrow
        // across egui calls.
        let rows: Vec<Row> = match world.get_resource::<ExperimentRegistry>() {
            Some(reg) => reg
                .list_for_twin(&twin)
                .iter()
                .map(|e| Row {
                    id: e.id,
                    name: e.name.clone(),
                    bounds: format!(
                        "{}→{}s · {}",
                        e.bounds.t_start,
                        e.bounds.t_end,
                        match e.bounds.dt {
                            Some(d) => format!("Δ{d}"),
                            None => "auto".into(),
                        }
                    ),
                    overrides: format_overrides_summary(&e.overrides),
                    status: status_label(&e.status),
                    duration_ms: match &e.status {
                        RunStatus::Done { wall_time_ms } => Some(*wall_time_ms),
                        _ => None,
                    },
                    error: matches!(e.status, RunStatus::Failed { .. })
                        .then(|| match &e.status {
                            RunStatus::Failed { error, .. } => error.clone(),
                            _ => String::new(),
                        }),
                    is_terminal: e.status.is_terminal(),
                    color_hint: e.color_hint,
                    sample_count: e
                        .result
                        .as_ref()
                        .map(|r| r.times.len())
                        .unwrap_or(0),
                    var_count: e
                        .result
                        .as_ref()
                        .map(|r| r.series.len())
                        .unwrap_or(0),
                    progress: match &e.status {
                        RunStatus::Running { t_current } => {
                            let span = (e.bounds.t_end - e.bounds.t_start).max(1e-9);
                            Some(
                                (((t_current - e.bounds.t_start) / span)
                                    .clamp(0.0, 1.0)) as f32,
                            )
                        }
                        _ => None,
                    },
                })
                .collect(),
            None => Vec::new(),
        };

        if rows.is_empty() {
            // Detect *why* the experiments table is empty so the
            // hint actually matches the user's situation. Without
            // this, the panel says "Press ⏩ Run above" even when
            // ⏩ Run was hidden by `render_setup_section` (no doc /
            // no class), leaving the user pointing at empty space.
            let active_doc = world
                .get_resource::<lunco_workbench::WorkspaceResource>()
                .and_then(|w| w.active_document);
            let has_class = active_doc
                .and_then(|doc| {
                    world
                        .get_resource::<crate::ui::state::ModelicaDocumentRegistry>()
                        .and_then(|r| r.host(doc))
                        .map(|h| {
                            h.document().index().classes.values().any(|c| {
                                !matches!(c.kind, crate::index::ClassKind::Package)
                            })
                        })
                })
                .unwrap_or(false);

            ui.vertical_centered(|ui| {
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new("Experiments")
                        .size(13.0)
                        .strong(),
                );
                ui.add_space(2.0);
                ui.weak(
                    "Fast Run — batch simulate a model end-to-end with chosen \
                     bounds and parameter overrides; each run is recorded \
                     below and can be overlaid in plots. For live tweaking \
                     while the model runs, use 🚀 Compile on the model \
                     toolbar (Interactive realtime mode).",
                );
                ui.add_space(8.0);

                match (active_doc, has_class) {
                    (None, _) => {
                        ui.weak("① Open a model — pick one in the Files panel,");
                        ui.weak("   or use File → New / Open Example.");
                        ui.weak("② A ⏩ Run button appears above once a model is active.");
                    }
                    (Some(_), false) => {
                        ui.weak(
                            "Active document has no model class yet. Switch to \
                             the 📝 Text view and add a `model Foo … end Foo;`,",
                        );
                        ui.weak("then return here to set bounds and click ⏩ Run.");
                    }
                    (Some(_), true) => {
                        ui.weak(
                            "▶ Press ⏩ Run above (or the ⏩ Fast button on the \
                             model toolbar) to start your first experiment.",
                        );
                        ui.add_space(2.0);
                        ui.weak(
                            "Pick variables in the Telemetry panel — they \
                             appear in the plot below the table.",
                        );
                    }
                }
            });
            return;
        }

        ui.horizontal(|ui| {
            ui.weak(format!("{} experiment(s)", rows.len()));
            // Surface the most recent terminal run's outcome inline
            // so users get clear "did it finish?" feedback without
            // hunting in Console. Picks the last Done/Failed/Cancelled
            // by registry insertion order (rows are appended).
            if let Some(last) = rows
                .iter()
                .rev()
                .find(|r| r.is_terminal)
            {
                ui.separator();
                let (txt, color) = if let Some(_err) = &last.error {
                    (format!("⚠ {} failed", last.name), col_error)
                } else if let Some(ms) = last.duration_ms {
                    (format!("✓ {} done in {} ms", last.name, ms), col_success)
                } else {
                    (format!("⊘ {} cancelled", last.name), col_subdued)
                };
                ui.label(egui::RichText::new(txt).color(color).strong());
            }
        });
        ui.separator();

        let mut toggle: Option<ExperimentId> = None;
        let mut delete: Option<ExperimentId> = None;
        let mut cancel: Option<ExperimentId> = None;
        // Selected row → load its setup into the draft. Right-click
        // gives Re-run / Duplicate. Both work on terminal rows; for
        // running rows ⊘ Cancel is the only useful action.
        let mut load_into_draft: Option<ExperimentId> = None;
        let mut rerun: Option<ExperimentId> = None;
        let mut export_csv: Option<ExperimentId> = None;
        // Inline rename state changes batched after Grid::show to
        // avoid double-borrow of ExperimentVisibility.
        let mut start_rename: Option<(ExperimentId, String)> = None;
        let mut commit_rename: Option<(ExperimentId, String)> = None;
        let mut cancel_rename = false;
        let editing_now = world
            .get_resource::<ExperimentVisibility>()
            .and_then(|v| v.editing_name.clone());

        // Table grid renders directly; the outer panel ScrollArea
        // wraps the whole panel including this grid.
        egui::Grid::new("experiments_table")
                .num_columns(7)
                .striped(true)
                .show(ui, |ui| {
                    ui.weak("👁").on_hover_text(
                        "Visibility in the currently focused plot tab. \
                         Each Plot Window keeps its own checked-runs set, \
                         so a checkbox here only affects the active plot. \
                         Switch plot tabs to manage another plot's set.",
                    );
                    ui.weak("Color");
                    ui.weak("Name");
                    ui.weak("Bounds");
                    ui.weak("Status");
                    ui.weak("Samples");
                    ui.weak("");
                    ui.end_row();

                    // The checkbox column toggles visibility *in the
                    // user's current plot* (target_plot pin if set,
                    // else the most-recently-rendered plot). Per-plot
                    // visibility lets each Plot Window show a
                    // different subset of runs — OMEdit / Dymola style.
                    let target_viz = {
                        let pinned = world
                            .get_resource::<ExperimentVisibility>()
                            .and_then(|v| v.target_plot);
                        pinned.unwrap_or_else(|| {
                            world
                                .get_resource::<ActivePlot>()
                                .copied()
                                .unwrap_or_default()
                                .or_default()
                        })
                    };
                    let visibility_snapshot: std::collections::HashSet<ExperimentId> = world
                        .get_resource::<PlotPanelStates>()
                        .map(|s| s.visible(target_viz))
                        .unwrap_or_default();

                    for row in &rows {
                        let mut visible = visibility_snapshot.contains(&row.id);
                        if ui.checkbox(&mut visible, "").changed() {
                            toggle = Some(row.id);
                        }
                        let (r, g, b) = palette_color(row.color_hint);
                        ui.colored_label(
                            egui::Color32::from_rgb(r, g, b),
                            "■",
                        );
                        // Name cell — either a TextEdit (inline rename
                        // active for this row) or a clickable Label.
                        // Click loads draft; right-click opens context
                        // menu including ✏ Rename.
                        let is_editing = matches!(&editing_now, Some((eid, _)) if *eid == row.id);
                        if is_editing {
                            let mut buf = match &editing_now {
                                Some((_, t)) => t.clone(),
                                None => row.name.clone(),
                            };
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut buf)
                                    .desired_width(140.0),
                            );
                            resp.request_focus();
                            let enter = resp.lost_focus()
                                && resp.ctx.input(|i| i.key_pressed(egui::Key::Enter));
                            let escape = resp.ctx.input(|i| i.key_pressed(egui::Key::Escape));
                            if enter || (resp.lost_focus() && !escape) {
                                let trimmed = buf.trim().to_string();
                                if !trimmed.is_empty() {
                                    commit_rename = Some((row.id, trimmed));
                                } else {
                                    cancel_rename = true;
                                }
                            } else if escape {
                                cancel_rename = true;
                            } else {
                                start_rename = Some((row.id, buf));
                            }
                        } else {
                            let name_label = egui::Label::new(&row.name)
                                .sense(egui::Sense::click());
                            let name_resp = ui
                                .add(name_label)
                                .on_hover_text(
                                    "Click: load this run's setup into the draft. \
                                     Double-click or right-click → Rename. \
                                     Right-click: Re-run / Duplicate / Delete.",
                                );
                            if name_resp.double_clicked() {
                                start_rename = Some((row.id, row.name.clone()));
                            } else if name_resp.clicked() && row.is_terminal {
                                load_into_draft = Some(row.id);
                            }
                            name_resp.context_menu(|ui| {
                                if ui.button("✏ Rename").on_hover_text("Give this run a new name").clicked() {
                                    start_rename = Some((row.id, row.name.clone()));
                                    ui.close();
                                }
                                ui.separator();
                                if row.is_terminal {
                                    if ui.button("▶ Re-run with same setup").on_hover_text("Run again with identical bounds and parameter overrides").clicked() {
                                        rerun = Some(row.id);
                                        ui.close();
                                    }
                                    if ui.button("📋 Duplicate into Setup").on_hover_text("Load this run's setup into the draft so you can tweak it").clicked() {
                                        load_into_draft = Some(row.id);
                                        ui.close();
                                    }
                                    if ui
                                        .button("💾 Export CSV…")
                                        .on_hover_text(
                                            "Save this run's full trajectory \
                                             (time + every recorded variable) \
                                             to a CSV file.",
                                        )
                                        .clicked()
                                    {
                                        export_csv = Some(row.id);
                                        ui.close();
                                    }
                                    ui.separator();
                                    if ui.button("✕ Delete").on_hover_text("Remove this run from the list").clicked() {
                                        delete = Some(row.id);
                                        ui.close();
                                    }
                                } else if ui.button("⊘ Cancel run").on_hover_text("Stop this in-progress run").clicked() {
                                    cancel = Some(row.id);
                                    ui.close();
                                }
                            });
                        }
                        ui.horizontal(|ui| {
                            ui.label(&row.bounds);
                            if !row.overrides.is_empty() {
                                ui.label(
                                    egui::RichText::new(format!("· {}", row.overrides))
                                        .color(col_warning)
                                        .small(),
                                )
                                .on_hover_text("Parameter overrides applied to this run");
                            }
                        });
                        // Color-code status: failed → red, cancelled →
                        // muted, running → amber, done → default.
                        let status_color = match (&row.error, row.is_terminal, row.duration_ms) {
                            (Some(_), _, _) => Some(col_error),
                            (None, false, None) => Some(col_warning),
                            _ => None,
                        };
                        let status_text = match status_color {
                            Some(c) => egui::RichText::new(&row.status).color(c),
                            None => egui::RichText::new(&row.status),
                        };
                        let status_widget = ui.horizontal(|ui| {
                            let r = ui.label(status_text);
                            if let Some(p) = row.progress {
                                ui.add(
                                    egui::ProgressBar::new(p)
                                        .desired_width(60.0)
                                        .desired_height(8.0),
                                )
                                .on_hover_text(format!("{:.0}%", p * 100.0));
                            }
                            r
                        }).inner;
                        if let Some(err) = &row.error {
                            status_widget.on_hover_text(err);
                        }
                        let sample_text = if row.var_count > 0 {
                            format!("{}×{}", row.sample_count, row.var_count)
                        } else {
                            String::new()
                        };
                        let sample_resp = ui.label(&sample_text);
                        if row.var_count > 0 {
                            sample_resp.on_hover_text(format!(
                                "{} samples × {} variables",
                                row.sample_count, row.var_count
                            ));
                        }
                        if row.is_terminal {
                            if ui.small_button("✕").on_hover_text("Delete").clicked() {
                                delete = Some(row.id);
                            }
                        } else {
                            if ui
                                .small_button("⊘")
                                .on_hover_text("Cancel run")
                                .clicked()
                            {
                                cancel = Some(row.id);
                            }
                        }
                        ui.end_row();
                    }
                });

        // Apply rename state transitions in priority order: commit
        // wins over cancel wins over start. Avoids flicker when a
        // single frame sees both an Enter (commit) and a focus-loss.
        if let Some((id, new_name)) = commit_rename {
            if let Some(mut reg) = world.get_resource_mut::<ExperimentRegistry>() {
                if let Some(exp) = reg.get_mut(id) {
                    exp.name = new_name;
                }
            }
            if let Some(mut v) = world.get_resource_mut::<ExperimentVisibility>() {
                v.editing_name = None;
            }
        } else if cancel_rename {
            if let Some(mut v) = world.get_resource_mut::<ExperimentVisibility>() {
                v.editing_name = None;
            }
        } else if let Some(state) = start_rename {
            if let Some(mut v) = world.get_resource_mut::<ExperimentVisibility>() {
                v.editing_name = Some(state);
            }
        }

        if let Some(id) = toggle {
            // Toggle visibility on whichever plot is currently active /
            // pinned. Each plot tab keeps its own visible set.
            let target_viz = {
                let pinned = world
                    .get_resource::<ExperimentVisibility>()
                    .and_then(|v| v.target_plot);
                pinned.unwrap_or_else(|| {
                    world
                        .get_resource::<ActivePlot>()
                        .copied()
                        .unwrap_or_default()
                        .or_default()
                })
            };
            if let Some(mut s) = world.get_resource_mut::<PlotPanelStates>() {
                s.toggle_visible(target_viz, id);
            }
        }
        if let Some(id) = delete {
            if let Some(mut reg) = world.get_resource_mut::<ExperimentRegistry>() {
                reg.delete(id);
            }
            if let Some(mut s) = world.get_resource_mut::<PlotPanelStates>() {
                s.forget_experiment(id);
            }
        }
        if let Some(id) = cancel {
            // Best-effort cancel via the runner's RunHandle. The
            // PendingHandles drain system will see the resulting
            // RunUpdate::Cancelled and update registry status.
            if let Some(handles) = world
                .get_resource::<crate::experiments_runner::PendingHandles>()
            {
                for h in &handles.0 {
                    if h.run_id == id {
                        h.cancel();
                        break;
                    }
                }
            }
        }
        if let Some(id) = load_into_draft {
            load_run_into_draft(world, id);
        }
        if let Some(id) = export_csv {
            export_experiment_csv(world, id);
        }
        if let Some(id) = rerun {
            // Load setup, then dispatch a new Fast Run with it.
            // Resolving the originating doc keeps diagnostics routed
            // back to the right tab.
            load_run_into_draft(world, id);
            if let Some(doc) = world
                .get_resource::<crate::experiments_runner::ExperimentSources>()
                .and_then(|s| s.0.get(&id).copied())
                .or_else(|| {
                    world
                        .get_resource::<lunco_workbench::WorkspaceResource>()
                        .and_then(|ws| ws.active_document)
                })
            {
                world
                    .commands()
                    .trigger(crate::ui::commands::FastRunActiveModel { doc });
            }
        }

        // Plot + variable picker now live in the Graphs panel — this
        // panel is the run *list* / comparison-source. See the Source
        // toggle in panels::graphs.
            }); // outer experiments_panel_scroll
    }
}

impl ExperimentsPanel {
    /// Persistent Setup section at the top of the Experiments panel.
    /// Compact bounds + inputs + Run button. Edits persist into the
    /// per-`ModelRef` draft; the toolbar's ⏩ Fast button reads the
    /// same draft, so changes here are visible there immediately.
    fn render_setup_section(&self, ui: &mut egui::Ui, world: &mut World) {
        let col_error = world.resource::<lunco_theme::Theme>().tokens.error;
        // Resolve target doc + model class. Honor the experiments
        // pin so a pinned panel keeps its setup form while the user
        // edits a different tab.
        let Some(doc) = crate::ui::doc_pin::resolved_experiments_doc(world)
        else {
            return;
        };
        let (model_name, source) = match world
            .get_resource::<crate::ui::state::ModelicaDocumentRegistry>()
            .and_then(|r| r.host(doc))
        {
            Some(h) => {
                let document = h.document();
                let drilled = world
                    .get_resource::<crate::ui::panels::model_view::ModelTabs>()
                    .and_then(|t| t.drilled_class_for_doc(doc));
                let first_non_pkg = document
                    .index()
                    .classes
                    .values()
                    .find(|c| !matches!(c.kind, crate::index::ClassKind::Package))
                    .map(|c| c.name.clone());
                let class = drilled.or(first_non_pkg);
                match class {
                    Some(c) => (c, document.source().to_string()),
                    None => return,
                }
            }
            None => return,
        };
        let model_ref = lunco_experiments::ModelRef(model_name.clone());

        // Snapshot draft + runner defaults for prefill.
        let draft_bounds = world
            .get_resource::<crate::experiments_runner::ExperimentDrafts>()
            .and_then(|d| d.get(doc, &model_ref).and_then(|dr| dr.bounds_override.clone()));
        let mut bounds = draft_bounds.unwrap_or_else(|| {
            world
                .get_resource::<crate::ModelicaRunnerResource>()
                .and_then(|r| {
                    use lunco_experiments::ExperimentRunner;
                    r.0.default_bounds(&model_ref)
                })
                .unwrap_or(lunco_experiments::RunBounds {
                    t_start: 0.0,
                    t_end: 10.0,
                    dt: None,
                    tolerance: None,
                    solver: None,
                })
        });
        let mut bounds_changed = false;

        let detected_inputs =
            crate::experiments_runner::detect_top_level_inputs(&source);
        let prefilled_inputs: BTreeMap<lunco_experiments::ParamPath, lunco_experiments::ParamValue> =
            world
                .get_resource::<crate::experiments_runner::ExperimentDrafts>()
                .and_then(|d| d.get(doc, &model_ref).map(|dr| dr.inputs.clone()))
                .unwrap_or_default();
        // Maintain editable text per input row across frames via a
        // local scratch in the panel — simpler than yet another
        // resource. Reset when model changes.
        let mut input_edits: Vec<(String, String, String)> = detected_inputs
            .iter()
            .map(|d| {
                let txt = prefilled_inputs
                    .get(&lunco_experiments::ParamPath(d.name.clone()))
                    .map(|v| match v {
                        lunco_experiments::ParamValue::Real(x) => format!("{x}"),
                        lunco_experiments::ParamValue::Int(x) => format!("{x}"),
                        lunco_experiments::ParamValue::Bool(b) => {
                            if *b { "true".into() } else { "false".into() }
                        }
                        lunco_experiments::ParamValue::String(s) => s.clone(),
                        lunco_experiments::ParamValue::Enum(s) => s.clone(),
                        lunco_experiments::ParamValue::RealArray(_) => "(array)".into(),
                    })
                    .unwrap_or_default();
                (d.name.clone(), d.type_name.clone(), txt)
            })
            .collect();
        let mut inputs_changed = false;
        let mut run_clicked = false;

        let runner_busy = world
            .get_resource::<crate::ModelicaRunnerResource>()
            .map(|r| r.0.is_busy())
            .unwrap_or(false);

        // Annotation-default reference for "is this what the model
        // says?" tagging next to the bounds inputs.
        let annotation_defaults = world
            .get_resource::<crate::ModelicaRunnerResource>()
            .and_then(|r| {
                use lunco_experiments::ExperimentRunner;
                r.0.default_bounds(&model_ref)
            });
        let from_annotation = annotation_defaults.is_some();

        // Header row stays always visible — Run + Cancel + a tiny
        // ▾ chip toggles the bounds/inputs detail section. This keeps
        // the table area maximised when the dock is shrunk.
        let mut cancel_active = false;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("Setup — {}", model_name)).strong())
                .on_hover_text("Bounds + inputs apply to the next run from this model.");
            if from_annotation {
                ui.weak("· bounds default from experiment(...) annotation");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if runner_busy
                    && ui
                        .small_button("⊘ Cancel")
                        .on_hover_text("Stop the current run.")
                        .clicked()
                {
                    cancel_active = true;
                }
                let label = if runner_busy { "⏩ Running…" } else { "⏩ Run" };
                let valid = bounds.t_end > bounds.t_start;
                let btn = ui.add_enabled(valid && !runner_busy, egui::Button::new(label));
                let btn = if runner_busy {
                    btn.on_disabled_hover_text(
                        "A run is already in progress — use ⊘ Cancel.",
                    )
                } else if !valid {
                    btn.on_disabled_hover_text(
                        "Bounds invalid — t_end must be greater than t_start.",
                    )
                } else {
                    btn.on_hover_text(
                        "Fast Run — compile + simulate end-to-end from t_start \
                         to t_end as fast as possible (no realtime, no live \
                         parameter edits). Result lands as a new row below; \
                         use 🚀 Compile on the model toolbar instead if you \
                         want interactive realtime stepping.",
                    )
                };
                if btn.clicked() {
                    run_clicked = true;
                }
                ui.label(format!("t: {:.2}→{:.2}s", bounds.t_start, bounds.t_end));
            });
        });
        if bounds.t_end <= bounds.t_start {
            ui.label(
                egui::RichText::new("⚠ t_end must be greater than t_start")
                    .color(col_error)
                    .size(11.0),
            );
        }

        // Bounds + inputs live behind a collapsing chip so the table
        // area gets the panel's vertical space by default. The header
        // already shows t_start→t_end inline so users see the active
        // bounds without expanding.
        let detail_label = if input_edits.is_empty() {
            "bounds".to_string()
        } else {
            format!("bounds + {} input{}", input_edits.len(), if input_edits.len() == 1 { "" } else { "s" })
        };
        egui::CollapsingHeader::new(detail_label)
            .id_salt("setup_detail")
            .default_open(true)
            .show(ui, |ui| {

        // Compact bounds row.
        ui.horizontal(|ui| {
            ui.label("t:");
            if ui.add(egui::DragValue::new(&mut bounds.t_start).speed(0.1)).changed() {
                bounds_changed = true;
            }
            ui.label("→");
            if ui.add(egui::DragValue::new(&mut bounds.t_end).speed(0.1)).changed() {
                bounds_changed = true;
            }
            ui.label("s");
            ui.separator();
            let mut adaptive = bounds.dt.is_none();
            let mut dt_v = bounds.dt.unwrap_or(0.01);
            if ui.checkbox(&mut adaptive, "adaptive dt").changed() {
                bounds.dt = if adaptive { None } else { Some(0.01) };
                bounds_changed = true;
            }
            if !adaptive
                && ui
                    .add(
                        egui::DragValue::new(&mut dt_v)
                            .speed(0.001)
                            .range(1e-6..=10.0),
                    )
                    .changed()
            {
                bounds.dt = Some(dt_v);
                bounds_changed = true;
            }

            // Solver picker. rumoca exposes three modes via
            // `SimSolverMode::from_external_name`: "auto" → Auto
            // (heuristic chooser), names containing "rk"/"dopri"/
            // "esdirk"/"trbdf2"/"euler"/"midpoint" → RkLike (explicit
            // Runge-Kutta family for non-stiff systems), everything
            // else → Bdf (implicit, default for stiff DAEs).
            // Stored as `Option<String>`; `None` means "use the
            // experiment(...) annotation, otherwise rumoca's default
            // (BDF)". See rumoca-sim/src/with_diffsol/mod.rs:90.
            ui.separator();
            ui.label("solver:")
                .on_hover_text(
                    "Integration method. Auto picks based on problem \
                     stiffness. BDF is implicit (good for stiff DAEs — \
                     thermal, chemical, electrical networks). RK4 is \
                     explicit Runge-Kutta (faster on non-stiff problems \
                     like rigid-body mechanics).",
                );
            let current: String = bounds.solver.clone().unwrap_or_else(|| "auto".to_string());
            let current = current.as_str();
            let label = match current {
                "auto" => "Auto",
                "bdf" => "BDF (stiff)",
                "rk4" => "RK4 (non-stiff)",
                other => other,
            };
            egui::ComboBox::from_id_salt("setup_solver")
                .selected_text(label)
                .width(140.0)
                .show_ui(ui, |ui| {
                    for (val, label, hover) in [
                        ("auto", "Auto",
                         "Let the backend pick based on stiffness heuristics."),
                        ("bdf", "BDF (stiff)",
                         "Backward Differentiation Formula — implicit, robust \
                          on stiff DAEs (thermal, chemical, electrical). \
                          Slower per step but stable with large dt."),
                        ("rk4", "RK4 (non-stiff)",
                         "Explicit Runge-Kutta — fast on smooth, non-stiff \
                          problems (rigid-body mechanics, kinematics). \
                          Can blow up on stiff systems."),
                    ] {
                        let resp = ui
                            .selectable_label(current == val, label)
                            .on_hover_text(hover);
                        if resp.clicked() {
                            bounds.solver = if val == "auto" {
                                None
                            } else {
                                Some(val.to_string())
                            };
                            bounds_changed = true;
                        }
                    }
                });
        });

        // Inputs row(s). Wrap horizontally — a model with many
        // inputs scrolls instead of growing vertically.
        if !input_edits.is_empty() {
            egui::ScrollArea::horizontal()
                .id_salt("setup_inputs_scroll")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.weak("Inputs:")
                            .on_hover_text(
                                "Values bound to top-level `input` declarations \
                                 before the run. Real → number; Boolean → \
                                 true/false; Integer → number. Empty cells use \
                                 the model's default.",
                            );
                        for (name, ty, value_text) in input_edits.iter_mut() {
                            ui.label(name.as_str());
                            let s_trim = value_text.trim();
                            let parses = if s_trim.is_empty() {
                                true
                            } else {
                                match ty.as_str() {
                                    "Real" => s_trim.parse::<f64>().is_ok(),
                                    "Integer" | "Int" => s_trim.parse::<i64>().is_ok(),
                                    "Boolean" | "Bool" => {
                                        matches!(s_trim, "true" | "false")
                                    }
                                    _ => s_trim.parse::<f64>().is_ok(),
                                }
                            };
                            let mut edit = egui::TextEdit::singleline(value_text)
                                .desired_width(70.0);
                            if !parses {
                                edit = edit.text_color(col_error);
                            }
                            let resp = ui.add(edit);
                            let resp = if !parses {
                                resp.on_hover_text(format!(
                                    "Cannot parse as {ty}. Expected: {}",
                                    match ty.as_str() {
                                        "Real" => "decimal number, e.g. 1.5",
                                        "Integer" | "Int" => "integer, e.g. 42",
                                        "Boolean" | "Bool" => "true or false",
                                        _ => "decimal number",
                                    }
                                ))
                            } else {
                                resp
                            };
                            if resp.changed() || resp.lost_focus() {
                                inputs_changed = true;
                            }
                        }
                    });
                });
        }
            }); // end CollapsingHeader

        // Wire the inline ⊘ Cancel button to the runner.
        if cancel_active {
            // Latest in-flight handle.
            if let Some(handles) = world
                .get_resource::<crate::experiments_runner::PendingHandles>()
            {
                if let Some(h) = handles.0.last() {
                    h.cancel();
                }
            }
        }

        // Persist edits.
        if bounds_changed {
            if let Some(mut drafts) = world
                .get_resource_mut::<crate::experiments_runner::ExperimentDrafts>()
            {
                drafts.entry(doc, model_ref.clone()).bounds_override = Some(bounds);
            }
        }
        if inputs_changed {
            // Build a new BTreeMap from edited text.
            let mut map: BTreeMap<lunco_experiments::ParamPath, lunco_experiments::ParamValue> =
                BTreeMap::new();
            for (name, ty, text) in input_edits.iter() {
                let s = text.trim();
                if s.is_empty() {
                    continue;
                }
                let v = match ty.as_str() {
                    "Real" => s.parse::<f64>().ok().map(lunco_experiments::ParamValue::Real),
                    "Integer" | "Int" => s.parse::<i64>().ok().map(lunco_experiments::ParamValue::Int),
                    "Boolean" | "Bool" => match s {
                        "true" => Some(lunco_experiments::ParamValue::Bool(true)),
                        "false" => Some(lunco_experiments::ParamValue::Bool(false)),
                        _ => None,
                    },
                    _ => s.parse::<f64>().ok().map(lunco_experiments::ParamValue::Real),
                };
                if let Some(v) = v {
                    map.insert(lunco_experiments::ParamPath(name.clone()), v);
                }
            }
            if let Some(mut drafts) = world
                .get_resource_mut::<crate::experiments_runner::ExperimentDrafts>()
            {
                drafts.entry(doc, model_ref).inputs = map;
            }
        }
        if run_clicked {
            // Skip the modal — Setup is already filled in.
            world
                .commands()
                .trigger(crate::ui::commands::FastRunActiveModel { doc });
        }
    }

    /// Override + bounds editor for the currently active document's
    /// top-level model. Detects literal `parameter` declarations in
    /// the source and shows them as an editable table; non-literal
    /// params appear greyed with a tooltip.
    fn render_override_editor(&self, ui: &mut egui::Ui, world: &mut World) {
        let Some(doc) = crate::ui::doc_pin::resolved_experiments_doc(world)
        else {
            return;
        };
        let registry = match world.get_resource::<crate::ui::state::ModelicaDocumentRegistry>() {
            Some(r) => r,
            None => return,
        };
        let host = match registry.host(doc) {
            Some(h) => h,
            None => return,
        };
        let document = host.document();
        let source = document.source().to_string();

        // Resolve the model class via the same path the Setup section
        // uses (drilled class → first non-package fallback) so this
        // section stays visible even when `strict_ast()` returns None
        // because of a recoverable parse warning.
        let drilled = world
            .get_resource::<crate::ui::panels::model_view::ModelTabs>()
            .and_then(|t| t.drilled_class_for_doc(doc));
        let first_non_pkg = document
            .index()
            .classes
            .values()
            .find(|c| !matches!(c.kind, crate::index::ClassKind::Package))
            .map(|c| c.name.clone());
        let Some(model_name) = drilled.or(first_non_pkg) else {
            return;
        };
        let model_ref = lunco_experiments::ModelRef(model_name.clone());

        let detected =
            crate::experiments_runner::detect_top_level_literal_parameters(&source);
        if detected.is_empty() {
            return;
        }

        egui::CollapsingHeader::new(format!(
            "⚙ Parameter overrides ({})",
            detected.iter().filter(|p| p.supportable).count()
        ))
            .id_salt("experiments_parameter_overrides")
            .default_open(false)
            .show(ui, |ui| {
                use lunco_experiments::{ParamPath, ParamValue};

                // Parameter overrides
                let current_overrides: BTreeMap<ParamPath, ParamValue> = world
                    .get_resource::<crate::experiments_runner::ExperimentDrafts>()
                    .and_then(|d| d.get(doc, &model_ref).map(|dr| dr.overrides.clone()))
                    .unwrap_or_default();

                let mut updates: Vec<(ParamPath, Option<ParamValue>)> = Vec::new();

                egui::Grid::new("override_grid")
                    .num_columns(4)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.weak("Type");
                        ui.weak("Name");
                        ui.weak("Default");
                        ui.weak("Override");
                        ui.end_row();

                        for p in &detected {
                            ui.label(&p.type_name);
                            ui.label(&p.name);
                            ui.label(p.default_literal.as_deref().unwrap_or("—"));
                            let path = ParamPath(p.name.clone());
                            if !p.supportable {
                                ui.add_enabled(
                                    false,
                                    egui::TextEdit::singleline(&mut String::from("—"))
                                        .desired_width(80.0),
                                )
                                .on_hover_text(
                                    p.reason
                                        .clone()
                                        .unwrap_or_else(|| "unsupported".into()),
                                );
                            } else {
                                // No-override state shows an *empty*
                                // editable cell with the default as
                                // hint text. Previously the field was
                                // pre-filled with the default literal,
                                // which made it indistinguishable from
                                // a disabled/read-only cell and users
                                // didn't realize they could click and
                                // type to override.
                                let existing = current_overrides.get(&path).cloned();
                                // Prefill with the current effective
                                // value — the override if set, else
                                // the model's default — so the user
                                // can modify in place (OMEdit-style)
                                // instead of clearing and retyping.
                                // The "×" button clears the override
                                // (revert to default).
                                let default_text = p
                                    .default_literal
                                    .clone()
                                    .unwrap_or_default();
                                let committed = match &existing {
                                    Some(ParamValue::Real(x)) => format!("{x}"),
                                    Some(ParamValue::Int(x)) => format!("{x}"),
                                    Some(ParamValue::Bool(b)) => {
                                        if *b { "true".into() } else { "false".into() }
                                    }
                                    Some(ParamValue::String(s)) => s.clone(),
                                    Some(ParamValue::Enum(s)) => s.clone(),
                                    Some(ParamValue::RealArray(_)) => "(array)".into(),
                                    None => default_text.clone(),
                                };
                                // Per-row id so egui can route keystrokes
                                // and preserve the in-progress edit buffer
                                // across frames. Without this the auto-id
                                // collides between rows that start with the
                                // same empty buffer and the cell silently
                                // rejects input.
                                let cell_id = egui::Id::new(("override_cell", p.name.as_str()));
                                // Latched draft: keeps typed characters
                                // alive across frames. Without this the
                                // local `text` re-initializes from the
                                // committed value every frame and wipes
                                // each keystroke.
                                let latched: Option<String> =
                                    ui.data_mut(|d| d.get_temp::<String>(cell_id));
                                let mut text = latched
                                    .clone()
                                    .unwrap_or_else(|| committed.clone());
                                let resp = ui.add(
                                    egui::TextEdit::singleline(&mut text)
                                        .id(cell_id)
                                        .desired_width(80.0),
                                );
                                if resp.has_focus() || resp.changed() {
                                    ui.data_mut(|d| d.insert_temp(cell_id, text.clone()));
                                }
                                // Compare against the *committed* value
                                // (last value pushed into the draft),
                                // not the latched in-progress text — the
                                // latch updates on every keystroke, so
                                // using it as the baseline makes
                                // `text != baseline` always false at
                                // focus-loss time and no commit fires
                                // unless the user explicitly pressed
                                // Enter.
                                let commit = resp.lost_focus()
                                    && (ui.input(|i| i.key_pressed(egui::Key::Enter))
                                        || text != committed);
                                if commit {
                                    // Typing back the unchanged default
                                    // text shouldn't materialise an
                                    // override — keep the row at "no
                                    // override set".
                                    let matches_default =
                                        existing.is_none() && text == default_text;
                                    if matches_default {
                                        ui.data_mut(|d| d.remove::<String>(cell_id));
                                    } else if let Some(v) =
                                        parse_override(&p.type_name, &text)
                                    {
                                        updates.push((path.clone(), Some(v)));
                                        // Latch the new value so the cell
                                        // keeps showing it until the draft
                                        // reflects the commit on the next
                                        // frame.
                                        ui.data_mut(|d| d.insert_temp(cell_id, text.clone()));
                                    } else if text.trim().is_empty() {
                                        updates.push((path.clone(), None));
                                        ui.data_mut(|d| d.remove::<String>(cell_id));
                                    }
                                } else if !resp.has_focus() {
                                    // Drop the latch once the committed
                                    // value catches up.
                                    if latched.as_deref() == Some(committed.as_str()) {
                                        ui.data_mut(|d| d.remove::<String>(cell_id));
                                    }
                                }
                                if existing.is_some() {
                                    if ui
                                        .small_button("×")
                                        .on_hover_text("Clear override")
                                        .clicked()
                                    {
                                        updates.push((path, None));
                                    }
                                }
                            }
                            ui.end_row();
                        }
                    });

                if !updates.is_empty() {
                    if let Some(mut drafts) = world
                        .get_resource_mut::<crate::experiments_runner::ExperimentDrafts>()
                    {
                        let entry = drafts.entry(doc, model_ref);
                        for (path, v) in updates {
                            match v {
                                Some(value) => {
                                    entry.overrides.insert(path, value);
                                }
                                None => {
                                    entry.overrides.remove(&path);
                                }
                            }
                        }
                    }
                }
            });
    }
}

fn parse_override(type_name: &str, text: &str) -> Option<lunco_experiments::ParamValue> {
    use lunco_experiments::ParamValue;
    let s = text.trim();
    if s.is_empty() {
        return None;
    }
    match type_name {
        "Real" => s.parse::<f64>().ok().map(ParamValue::Real),
        "Integer" | "Int" => s.parse::<i64>().ok().map(ParamValue::Int),
        "Boolean" | "Bool" => match s {
            "true" => Some(ParamValue::Bool(true)),
            "false" => Some(ParamValue::Bool(false)),
            _ => None,
        },
        "String" => {
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                Some(ParamValue::String(s[1..s.len() - 1].to_string()))
            } else {
                Some(ParamValue::String(s.to_string()))
            }
        }
        _ => {
            // Best-effort fallback: if it parses as a number, keep it
            // as Real. Otherwise treat as Enum literal name.
            if let Ok(x) = s.parse::<f64>() {
                Some(ParamValue::Real(x))
            } else {
                Some(ParamValue::Enum(s.to_string()))
            }
        }
    }
}

struct PlotSeries {
    label: String,
    color: (u8, u8, u8),
    points: Vec<[f64; 2]>,
    /// Stroke pattern that distinguishes runs sharing the same
    /// variable color. `0 = solid, 1 = dashed, 2 = dotted, 3 = dash-dot`.
    style_idx: u8,
}

/// Render the experiments multi-series plot. Picker lives in
/// Telemetry now; this just collects whatever variables Telemetry
/// has ticked + every visible experiment, builds series, and fills
/// the available space. v1 single-twin scope.
///
/// Variable units are pulled from the active doc's per-component
/// index (`modifications.get("unit")`) and surfaced two ways:
/// - Legend: `Run 1 · engine.thrust [N]`.
/// - Y-axis label: shows the unit when every visible variable shares
///   one; otherwise blank (mixed-unit plots happen often when users
///   tick variables across components).
/// Extra line injected into the experiments plot — used by
/// [`crate::ui::panels::graphs`] to overlay live `SignalRegistry`
/// histories on top of the completed-run curves so users see a
/// single merged plot instead of two stacked widgets.
pub struct PlotExtraLine {
    pub label: String,
    pub color: (u8, u8, u8),
    pub points: Vec<[f64; 2]>,
}

/// Render a bare plot frame plus any live overlays. Used when no
/// active doc is resolved so the Graphs tab still shows a plot
/// widget instead of disappearing.
fn render_empty_plot_frame(ui: &mut egui::Ui, extras: &[PlotExtraLine]) {
    Plot::new("graphs_experiments_plot_empty")
        .legend(Legend::default())
        .allow_drag(false)
        .show(ui, |plot_ui| {
            for ex in extras {
                let (r, g, b) = ex.color;
                let line =
                    Line::new(ex.label.clone(), PlotPoints::from(ex.points.clone()))
                        .color(egui::Color32::from_rgb(r, g, b));
                plot_ui.line(line);
            }
        });
}

pub fn render_experiments_plot(
    ui: &mut egui::Ui,
    world: &mut World,
    viz_id: VizId,
) -> ExpPlotSummary {
    render_experiments_plot_inner(ui, world, viz_id, &[])
}

pub fn render_experiments_plot_with_extras(
    ui: &mut egui::Ui,
    world: &mut World,
    viz_id: VizId,
    extras: &[PlotExtraLine],
) -> ExpPlotSummary {
    render_experiments_plot_inner(ui, world, viz_id, extras)
}

fn render_experiments_plot_inner(
    ui: &mut egui::Ui,
    world: &mut World,
    viz_id: VizId,
    extras: &[PlotExtraLine],
) -> ExpPlotSummary {
    // Scope to the experiments-pinned (or active) doc — same
    // semantics as the Experiments table above. When no doc is
    // resolved yet (boot, welcome screen, no model open) we still
    // render an empty plot widget plus any live overlays so the
    // Graphs tab never collapses to a blank panel.
    let Some(doc_id) = crate::ui::doc_pin::resolved_experiments_doc(world)
    else {
        // No doc resolved yet — draw just the doc badge. Action
        // buttons (New / Dup / Fit / CSV) live in the Graphs panel's
        // shared header, rendered above this body in every state.
        let col_muted = world.resource::<lunco_theme::Theme>().tokens.text_subdued;
        ui.label(
            egui::RichText::new("📈 (no model)  ·  0 vars")
                .color(col_muted)
                .small(),
        );
        render_empty_plot_frame(ui, extras);
        return ExpPlotSummary::default();
    };
    let twin = crate::ui::doc_pin::twin_id_for_doc(doc_id);
    let (col_warning, col_accent, col_muted) = {
        let t = world.resource::<lunco_theme::Theme>();
        (t.tokens.warning, t.tokens.accent, t.tokens.text_subdued)
    };

    // Doc switch → archive the previous twin's picks / visibility
    // and restore any prior stash for the new twin, so returning to
    // a tab brings back its plot selections instead of dropping them.
    if let Some(mut states) = world.get_resource_mut::<PlotPanelStates>() {
        states.sync_twin(viz_id, &twin);
    }

    // Doc badge so the user can tell which model's runs are
    // plotted (this plot inherits the Experiments panel's pin /
    // active-doc resolution; there's no per-plot pin).
    let doc_label = crate::ui::doc_pin::doc_display_name(world, doc_id);
    let run_count = world
        .get_resource::<ExperimentRegistry>()
        .map(|r| r.list_for_twin(&twin).len())
        .unwrap_or(0);

    let (visible, picked_vars) = world
        .get_resource::<PlotPanelStates>()
        .map(|s| (s.visible(viz_id), s.picked(viz_id)))
        .unwrap_or_default();

    // Build var -> unit map from the active doc index.
    let units: std::collections::HashMap<String, String> = active_doc_units(world, viz_id);

    let mut series: Vec<PlotSeries> = Vec::new();
    let mut total_runs = 0usize;
    let mut visible_runs = 0usize;
    let mut shared_unit: Option<String> = None;
    let mut shared_unit_init = false;
    // Stable per-variable index so each picked var gets a distinct
    // colour rotation regardless of run. Sort the picked set so the
    // mapping doesn't depend on insertion order.
    let var_idx: std::collections::HashMap<String, usize> = {
        let mut sorted: Vec<&String> = picked_vars.iter().collect();
        sorted.sort();
        sorted.into_iter().enumerate().map(|(i, s)| (s.clone(), i)).collect()
    };
    if let Some(reg) = world.get_resource::<ExperimentRegistry>() {
        for exp in reg.list_for_twin(&twin) {
            total_runs += 1;
            let Some(result) = &exp.result else { continue };
            if !visible.contains(&exp.id) {
                continue;
            }
            visible_runs += 1;
            for var in &picked_vars {
                if let Some(values) = result.series.get(var) {
                    let unit = units.get(var).cloned();
                    // Track shared-unit-across-series for the y-axis
                    // label; flip to None on first mismatch.
                    if !shared_unit_init {
                        shared_unit = unit.clone();
                        shared_unit_init = true;
                    } else if shared_unit != unit {
                        shared_unit = None;
                    }
                    // Truncate long dotted paths to keep the legend
                    // readable when nested components are picked.
                    // Keep the leaf + previous segment; collapse the
                    // rest as `…`.
                    let var_short = {
                        let parts: Vec<&str> = var.split('.').collect();
                        if parts.len() <= 2 {
                            var.clone()
                        } else {
                            format!("…{}.{}", parts[parts.len() - 2], parts[parts.len() - 1])
                        }
                    };
                    let label = match &unit {
                        Some(u) if !u.is_empty() => {
                            format!("{} · {} [{}]", exp.name, var_short, u)
                        }
                        _ => format!("{} · {}", exp.name, var_short),
                    };
                    let pts: Vec<[f64; 2]> = result
                        .times
                        .iter()
                        .zip(values.iter())
                        .map(|(t, y)| [*t, *y])
                        .collect();
                    // Convention: color = variable identity, line
                    // style = run identity. So `airframe.altitude`
                    // is always blue, but Run 1 = solid, Run 2 =
                    // dashed, Run 3 = dotted. Lets the eye track a
                    // variable across sweeps without legend hopping.
                    let v_idx = var_idx.get(var).copied().unwrap_or(0) as u8;
                    let color = palette_color(v_idx);
                    let style_idx = exp.color_hint % 4;
                    series.push(PlotSeries {
                        label,
                        color,
                        points: pts,
                        style_idx,
                    });
                }
            }
        }
    }

    let scrub_time = world
        .get_resource::<PlotPanelStates>()
        .and_then(|s| s.scrub(viz_id));

    let mut new_scrub: Option<Option<f64>> = None;

    // Inline variable picker — surfaces every variable known across
    // visible runs as a chip-style toggle row, so the user doesn't
    // need to hunt the Telemetry panel just to swap out a series.
    // Renders even when nothing is plotted yet so a fresh run lands
    // with an obvious "tick a chip" affordance.
    let mut all_vars: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Some(reg) = world.get_resource::<ExperimentRegistry>() {
        for exp in reg.list_for_twin(&twin) {
            if let Some(r) = &exp.result {
                for k in r.series.keys() {
                    all_vars.insert(k.clone());
                }
            }
        }
    }
    // Variable picker — Dymola / OMEdit-style component tree.
    // Variables group by their first dotted segment (the component
    // name). Each group is a CollapsingHeader; leaves are
    // checkboxes labelled with the leaf name. The whole tree sits
    // in a small horizontal scroll-row above the plot so the
    // common case (handful of components) reads at a glance and
    // scrolls horizontally on long models.
    // Picker tree + plot controls on a single line. Picker on the
    // left (component groups, expandable); reset / fit / mixed-units
    // chips right-aligned. Saves a row of vertical chrome above the
    // plot.
    let mut toggle_var: Option<String> = None;
    let mut reset_clicked = false;
    // Header — doc badge + var picker chips. Plot action buttons
    // (New / Dup / Fit / CSV) live in the Graphs panel's shared
    // header rendered above this body, so they stay reachable in
    // every state including the pure-live LinePlot mode.
    let var_count = picked_vars.len();
    let mut groups: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for v in &all_vars {
        let (head, tail) = match v.split_once('.') {
            Some((h, t)) => (h.to_string(), t.to_string()),
            None => (String::new(), v.clone()),
        };
        groups.entry(head).or_default().push(tail);
    }
    let group_count = groups.len();
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "📈 {doc_label}  ·  {var_count} var{}",
                if var_count == 1 { "" } else { "s" }
            ))
            .color(col_muted)
            .small(),
        );
        // Right-aligned action cluster first so the picker scroll
        // area gets the remaining middle space.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if scrub_time.is_some() {
                if ui
                    .small_button("↻")
                    .on_hover_text("Drop scrub cursor")
                    .clicked()
                {
                    reset_clicked = true;
                }
                if let Some(t) = scrub_time {
                    ui.label(
                        egui::RichText::new(format!("⏱ {t:.3}s"))
                            .size(11.0)
                            .monospace(),
                    );
                }
            }
            if shared_unit.is_none() && !series.is_empty() && picked_vars.len() > 1 {
                ui.label(
                    egui::RichText::new("⚠ mixed units")
                        .size(11.0)
                        .color(col_warning),
                )
                .on_hover_text("Picked variables have different units; y-axis label suppressed.");
            }
            // Middle/left: picker chips. Inside the right-to-left
            // layout but rendered as a horizontal-scroll area
            // consuming the remaining width.
            if !groups.is_empty() {
                egui::ScrollArea::horizontal()
                    .id_salt("exp_picker_scroll")
                    .max_height(20.0)
                    .show(ui, |ui| {
                        ui.with_layout(
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                for (head, tails) in &groups {
                                    let picked_in_group = tails.iter().filter(|t| {
                                        let full = if head.is_empty() {
                                            (*t).clone()
                                        } else {
                                            format!("{head}.{t}")
                                        };
                                        picked_vars.contains(&full)
                                    }).count();
                                    let label = if head.is_empty() {
                                        format!("(top) {}/{}", picked_in_group, tails.len())
                                    } else {
                                        format!("{head} {}/{}", picked_in_group, tails.len())
                                    };
                                    egui::CollapsingHeader::new(label)
                                        .id_salt(format!("exp_picker_group_{head}"))
                                        .default_open(group_count <= 1)
                                        .show(ui, |ui| {
                                            for t in tails {
                                                let full = if head.is_empty() {
                                                    t.clone()
                                                } else {
                                                    format!("{head}.{t}")
                                                };
                                                let mut on = picked_vars.contains(&full);
                                                if ui.checkbox(&mut on, t)
                                                    .on_hover_text(&full)
                                                    .changed()
                                                {
                                                    toggle_var = Some(full);
                                                }
                                            }
                                        });
                                }
                            },
                        );
                    });
            }
        });
    });
    if let Some(v) = toggle_var {
        if let Some(mut states) = world.get_resource_mut::<PlotPanelStates>() {
            states.toggle_var(viz_id, v);
        }
    }

    // Empty-state auto-promote — when this plot tab has no visible
    // runs but the doc *has* completed runs, automatically mark the
    // latest run visible and auto-pick top dynamic vars. Gated by
    // `auto_show_attempted` so clearing curves later doesn't re-fire
    // the promote on the next frame. The flag is reset on twin
    // switch (sync_twin restores a fresh state for new twins).
    let auto_show_pending = {
        let needs_auto = series.is_empty() && run_count > 0;
        if needs_auto {
            world
                .get_resource::<PlotPanelStates>()
                .and_then(|s| s.by_viz.get(&viz_id))
                .map(|st| !st.auto_show_attempted)
                .unwrap_or(true)
        } else {
            false
        }
    };
    let show_latest_clicked = auto_show_pending;

    // Drain any one-shot Fit request for this plot. The Graphs
    // panel's shared header queues it via `VizFitRequests`; the
    // LinePlot body drains the same resource, so Fit behaves
    // identically in both plot modes.
    let fit_requested = world
        .get_resource_mut::<lunco_viz::VizFitRequests>()
        .map(|mut r| r.take(viz_id))
        .unwrap_or(false);

    // Plot frame always renders. x-axis label dropped: time is
    // implicit in this panel and the label was burning a row of
    // pixels for one symbol.
    {
        let mut plot = Plot::new("graphs_experiments_plot")
            .legend(Legend::default())
            // Don't let the dragger eat clicks — we want clicks to set
            // the scrub cursor instead of pan/zoom. Box-zoom stays on
            // the modifier defaults; double-click still resets bounds.
            .allow_drag(false);
        if fit_requested {
            plot = plot.reset();
        }
        if let Some(u) = shared_unit.as_ref().filter(|u| !u.is_empty()) {
            plot = plot.y_axis_label(format!("[{u}]"));
        }
        let captured_x: std::cell::Cell<Option<f64>> = std::cell::Cell::new(None);
        plot.show(ui, |plot_ui| {
            for s in &series {
                let (r, g, b) = s.color;
                let style = match s.style_idx {
                    0 => LineStyle::Solid,
                    1 => LineStyle::dashed_dense(),
                    2 => LineStyle::dotted_dense(),
                    _ => LineStyle::dashed_loose(),
                };
                let line = Line::new(s.label.clone(), PlotPoints::from(s.points.clone()))
                    .color(egui::Color32::from_rgb(r, g, b))
                    .style(style);
                plot_ui.line(line);
            }
            // Live `SignalRegistry` curves overlaid on top of the
            // run curves so users get a single merged plot instead
            // of separate "experiment" and "live" widgets.
            for ex in extras {
                let (r, g, b) = ex.color;
                let line =
                    Line::new(ex.label.clone(), PlotPoints::from(ex.points.clone()))
                        .color(egui::Color32::from_rgb(r, g, b));
                plot_ui.line(line);
            }
            if let Some(t) = scrub_time {
                plot_ui.vline(
                    VLine::new("scrub", t)
                        .color(col_accent)
                        .width(1.5),
                );
            }
            // Click anywhere on the chart sets the scrub time. Drag
            // is disabled (allow_drag=false above) so clicks aren't
            // ambiguous with pan.
            if plot_ui.response().clicked() {
                if let Some(p) = plot_ui.pointer_coordinate() {
                    captured_x.set(Some(p.x));
                }
            }
        });
        if let Some(x) = captured_x.get() {
            new_scrub = Some(Some(x));
        }
    }

    if let Some(s) = new_scrub {
        if let Some(mut states) = world.get_resource_mut::<PlotPanelStates>() {
            states.set_scrub(viz_id, s);
        }
    }
    if show_latest_clicked {
        // Find the most recently completed run on this twin and
        // promote it: mark visible + auto-pick top-3 dynamic vars
        // by series variance (mirrors the auto-pick the runner
        // does on first completion).
        let latest = world
            .get_resource::<ExperimentRegistry>()
            .and_then(|reg| {
                reg.list_for_twin(&twin)
                    .into_iter()
                    .rev()
                    .find(|e| e.result.is_some())
                    .cloned()
            });
        if let Some(exp) = latest {
            if let Some(mut states) = world.get_resource_mut::<PlotPanelStates>() {
                let entry = states.entry(viz_id);
                entry.visible_experiments.insert(exp.id);
                entry.auto_show_attempted = true;
                if entry.picked_vars.is_empty() {
                    if let Some(result) = &exp.result {
                        let mut by_var: Vec<(&String, f64)> = result
                            .series
                            .iter()
                            .map(|(k, v)| {
                                let n = v.len().max(1) as f64;
                                let mean = v.iter().copied().sum::<f64>() / n;
                                let var = v
                                    .iter()
                                    .map(|x| (x - mean) * (x - mean))
                                    .sum::<f64>()
                                    / n;
                                (k, var)
                            })
                            .filter(|(_, v)| v.is_finite() && *v > 1e-12)
                            .collect();
                        by_var.sort_by(|a, b| {
                            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        for (k, _) in by_var.into_iter().take(3) {
                            entry.picked_vars.insert(k.clone());
                        }
                    }
                }
            }
        }
    }
    ExpPlotSummary {
        total_runs,
        visible_runs,
        series_drawn: series.len(),
        picked_vars: picked_vars.len(),
    }
}

/// Write a completed experiment's full trajectory to a user-picked
/// CSV file. Format: header `time,<var1>,<var2>,…` followed by one
/// row per sample. All variables share the run's `times` vector
/// already, so no resampling is needed (unlike the live-cosim CSV
/// export in the Graphs panel which has to merge per-signal histories).
///
/// Routes through `lunco_storage::FileStorage` so the same call site
/// will work when an OPFS / browser-download backend lands for wasm.
/// Cancelling the picker is a silent no-op; errors land in Console.
fn export_experiment_csv(world: &mut World, id: ExperimentId) {
    use lunco_storage::Storage as _;

    let (file_stem, csv_text) = {
        let registry = match world.get_resource::<ExperimentRegistry>() {
            Some(r) => r,
            None => return,
        };
        let Some(exp) = registry.get(id) else { return };
        let Some(result) = &exp.result else {
            if let Some(mut console) =
                world.get_resource_mut::<crate::ui::panels::console::ConsoleLog>()
            {
                console.error(
                    "CSV export: experiment has no result yet (still running or failed)",
                );
            }
            return;
        };
        let mut text = String::new();
        // Header row.
        text.push_str("time");
        let mut var_order: Vec<&String> = result.series.keys().collect();
        var_order.sort();
        for v in &var_order {
            text.push(',');
            // Quote names that contain commas / quotes; Modelica
            // dotted paths normally don't, but be defensive.
            push_csv_field(&mut text, v);
        }
        text.push('\n');
        // Data rows.
        for (i, t) in result.times.iter().enumerate() {
            text.push_str(&format!("{t}"));
            for v in &var_order {
                text.push(',');
                let val = result.series.get(*v).and_then(|col| col.get(i));
                match val {
                    Some(x) if x.is_finite() => text.push_str(&format!("{x}")),
                    _ => {} // empty cell for NaN / out-of-range
                }
            }
            text.push('\n');
        }
        // Filename suggestion: <model>_<run>_<unix_ts>. Unix seconds
        // is unambiguous across timezones and easy to glob; the run
        // name is included for readability when filing multiple
        // exports of the same model.
        let model_short = exp.model_ref.0.rsplit('.').next().unwrap_or(&exp.model_ref.0);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let raw = format!("{model_short}_{}_{ts}", exp.name);
        let safe_name: String = raw
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect();
        (safe_name, text)
    };

    let storage = lunco_storage::FileStorage::new();
    let hint = lunco_storage::SaveHint {
        suggested_name: Some(format!("{file_stem}.csv")),
        start_dir: None,
        filters: vec![lunco_storage::OpenFilter::new("CSV", &["csv"])],
    };
    let handle = match futures_lite::future::block_on(storage.pick_save(&hint)) {
        Ok(Some(h)) => h,
        Ok(None) => return,
        Err(e) => {
            if let Some(mut console) =
                world.get_resource_mut::<crate::ui::panels::console::ConsoleLog>()
            {
                console.error(format!("CSV export: picker failed: {e}"));
            }
            return;
        }
    };
    if let Err(e) = futures_lite::future::block_on(storage.write(&handle, csv_text.as_bytes())) {
        if let Some(mut console) =
            world.get_resource_mut::<crate::ui::panels::console::ConsoleLog>()
        {
            console.error(format!("CSV export: write failed: {e}"));
        }
    } else if let Some(mut console) =
        world.get_resource_mut::<crate::ui::panels::console::ConsoleLog>()
    {
        console.info(format!("✓ Exported experiment to {file_stem}.csv"));
    }
}

fn push_csv_field(out: &mut String, s: &str) {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        out.push('"');
        for c in s.chars() {
            if c == '"' {
                out.push('"');
            }
            out.push(c);
        }
        out.push('"');
    } else {
        out.push_str(s);
    }
}

/// Copy a completed experiment's bounds + inputs + overrides into
/// the per-`ModelRef` draft. The toolbar's bounds readout, the
/// inline Setup section, and the Setup modal all read from that
/// draft, so a row click is enough to "fork" a previous run as the
/// next setup. Pure World mutation; no event dispatched.
fn load_run_into_draft(world: &mut World, id: ExperimentId) {
    let snapshot = {
        let registry = match world.get_resource::<ExperimentRegistry>() {
            Some(r) => r,
            None => return,
        };
        registry.get(id).map(|e| (
                e.model_ref.clone(),
                e.bounds.clone(),
                e.inputs.clone(),
                e.overrides.clone(),
            ))
    };
    let Some((model_ref, bounds, inputs, overrides)) = snapshot else {
        return;
    };
    // Route the draft into the doc that originally spawned this run
    // (tracked in `ExperimentSources`). Fall back to the currently
    // resolved experiments doc if the source mapping is missing.
    let doc = world
        .get_resource::<crate::experiments_runner::ExperimentSources>()
        .and_then(|src| src.0.get(&id).copied())
        .or_else(|| crate::ui::doc_pin::resolved_experiments_doc(world));
    let Some(doc) = doc else { return };
    if let Some(mut drafts) = world
        .get_resource_mut::<crate::experiments_runner::ExperimentDrafts>()
    {
        let entry = drafts.entry(doc, model_ref);
        entry.bounds_override = Some(bounds);
        entry.inputs = inputs;
        entry.overrides = overrides;
    }
}

/// Build a `var_path -> unit` map for whatever the picker has
/// selected, by querying the active document's component index.
/// Walks `picked_vars` directly so the cost stays O(picks) instead
/// of O(all-components-in-the-model).
///
/// Uses [`ModelicaIndex::find_component_by_leaf`] so dotted paths
/// like `engine.thrust` resolve to a component declared somewhere
/// in the model with leaf name `thrust`. First match wins on
/// collisions across classes — same trade-off the rest of the UI
/// already makes.
fn active_doc_units(
    world: &World,
    viz_id: VizId,
) -> std::collections::HashMap<String, String> {
    let mut out: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let Some(doc) = world
        .get_resource::<lunco_workbench::WorkspaceResource>()
        .and_then(|ws| ws.active_document)
    else {
        return out;
    };
    let Some(registry) = world.get_resource::<crate::ui::state::ModelicaDocumentRegistry>()
    else {
        return out;
    };
    let Some(host) = registry.host(doc) else {
        return out;
    };
    let Some(picked) = world
        .get_resource::<PlotPanelStates>()
        .map(|s| s.picked(viz_id))
    else {
        return out;
    };
    let document = host.document();
    let index = document.index();
    for var in &picked {
        if let Some(entry) = index.find_component_by_leaf(var) {
            if let Some(unit) = entry.modifications.get("unit") {
                if !unit.is_empty() {
                    out.insert(var.clone(), unit.clone());
                }
            }
        }
    }
    out
}

/// Aggregate counters returned by [`render_experiments_plot`] so the
/// Graphs panel can fold them into its single header line instead of
/// rendering its own status text.
#[derive(Default)]
pub struct ExpPlotSummary {
    pub total_runs: usize,
    pub visible_runs: usize,
    pub series_drawn: usize,
    pub picked_vars: usize,
}

/// Compute an [`ExpPlotSummary`] without rendering. Lets the Graphs
/// panel show counts in its top header row before drawing the plot.
pub fn experiments_plot_summary(world: &World, viz_id: VizId) -> ExpPlotSummary {
    let Some(doc_id) = crate::ui::doc_pin::resolved_experiments_doc(world)
    else {
        return ExpPlotSummary::default();
    };
    let twin = crate::ui::doc_pin::twin_id_for_doc(doc_id);
    let (visible, picked_vars) = world
        .get_resource::<PlotPanelStates>()
        .map(|s| (s.visible(viz_id), s.picked(viz_id)))
        .unwrap_or_default();
    let mut total_runs = 0usize;
    let mut visible_runs = 0usize;
    let mut series_drawn = 0usize;
    if let Some(reg) = world.get_resource::<ExperimentRegistry>() {
        for exp in reg.list_for_twin(&twin) {
            total_runs += 1;
            let Some(result) = &exp.result else { continue };
            if !visible.contains(&exp.id) {
                continue;
            }
            visible_runs += 1;
            for var in &picked_vars {
                if result.series.contains_key(var) {
                    series_drawn += 1;
                }
            }
        }
    }
    ExpPlotSummary {
        total_runs,
        visible_runs,
        series_drawn,
        picked_vars: picked_vars.len(),
    }
}

/// Collect every variable name across all completed experiments for
/// the active twin. Used by the Telemetry panel to surface
/// experiment-only variables alongside live cosim signals.
pub fn all_experiment_variables(world: &World) -> std::collections::BTreeSet<String> {
    let mut out: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let Some(doc_id) = crate::ui::doc_pin::resolved_experiments_doc(world)
    else {
        return out;
    };
    let twin = crate::ui::doc_pin::twin_id_for_doc(doc_id);
    if let Some(reg) = world.get_resource::<ExperimentRegistry>() {
        for exp in reg.list_for_twin(&twin) {
            if let Some(result) = &exp.result {
                for k in result.series.keys() {
                    out.insert(k.clone());
                }
            }
        }
    }
    out
}

struct Row {
    id: ExperimentId,
    name: String,
    bounds: String,
    /// Comma-separated `name=value` for every override on this run.
    /// Empty when the run used the model's defaults. Shown in the
    /// Bounds column so users can scan which experiments deviated.
    overrides: String,
    status: String,
    duration_ms: Option<u64>,
    error: Option<String>,
    is_terminal: bool,
    color_hint: u8,
    sample_count: usize,
    var_count: usize,
    /// Progress fraction in `[0, 1]` while a run is in flight.
    /// `None` for terminal/pending rows. Drives the progress bar in
    /// the Status column so users get "how far along" without doing
    /// arithmetic against the bounds string.
    progress: Option<f32>,
}

fn format_overrides_summary(
    overrides: &std::collections::BTreeMap<
        lunco_experiments::ParamPath,
        lunco_experiments::ParamValue,
    >,
) -> String {
    use lunco_experiments::ParamValue;
    if overrides.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = overrides
        .iter()
        .map(|(path, v)| {
            let val = match v {
                ParamValue::Real(x) => format!("{x}"),
                ParamValue::Int(x) => format!("{x}"),
                ParamValue::Bool(b) => if *b { "true".into() } else { "false".into() },
                ParamValue::String(s) => format!("\"{s}\""),
                ParamValue::Enum(s) => s.clone(),
                ParamValue::RealArray(_) => "[…]".into(),
            };
            format!("{}={}", path.0, val)
        })
        .collect();
    parts.join(", ")
}

fn status_label(s: &RunStatus) -> String {
    match s {
        RunStatus::Pending => "⌛ Pending".into(),
        RunStatus::Running { t_current } => format!("▶ {t_current:.2}s"),
        RunStatus::Done { wall_time_ms } => format!("✓ Done ({wall_time_ms} ms)"),
        RunStatus::Failed { .. } => "⚠ Failed".into(),
        RunStatus::Cancelled => "⊘ Cancelled".into(),
    }
}

/// Stable color palette indexed by `Experiment.color_hint`. Keep
/// in sync with the Graphs panel's per-series color (Step 7).
pub fn palette_color(idx: u8) -> (u8, u8, u8) {
    // 8-color qualitative palette; cycles via modulo so the registry
    // cap (20) doesn't matter for color reuse.
    const PALETTE: &[(u8, u8, u8)] = &[
        (66, 133, 244),  // blue
        (219, 68, 55),   // red
        (244, 180, 0),   // amber
        (15, 157, 88),   // green
        (171, 71, 188),  // purple
        (255, 112, 67),  // orange
        (38, 166, 154),  // teal
        (236, 64, 122),  // pink
    ];
    PALETTE[idx as usize % PALETTE.len()]
}
