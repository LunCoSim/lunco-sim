//! UI-reactive observers of CORE state.
//!
//! These systems are the *reactive UI layer built on top of the core*: they
//! read core domain state (e.g. [`crate::msl_remote::MslLoadState`]) and
//! project it into UI surfaces (the workbench status bus, console, plots).
//! The core never references these surfaces — it just owns the observable
//! state. All of this is `ui`-feature only; a headless build has no observers
//! and therefore no egui/workbench dependency.

use bevy::prelude::*;
use lunco_workbench::status_bus::{StatusBus, StatusLevel};
use lunco_viz::{SignalMeta, SignalRef, SignalRegistry, VisualizationRegistry};

use lunco_assets::msl::{MslLoadPhase, MslLoadState};

const MSL_SOURCE: &str = "MSL";

/// Watch [`MslLoadState`] and translate transitions / progress ticks into
/// [`StatusBus`] events. Phase changes become discrete `Info` entries
/// (preserved in history); byte/file counts within a phase become `Progress`
/// ticks (updated in place).
///
/// This is a pure state mirror, not a task owner — `MslLoadState` itself is the
/// lifetime authority, so it uses the legacy `push_progress`/`clear_progress`
/// API (implicitly `BusyScope::Global`, matching MSL-preload-affects-everything
/// semantics) rather than `begin` + `BusyHandle`.
pub fn mirror_msl_state_to_status_bus(
    state: Res<MslLoadState>,
    bus: Option<ResMut<StatusBus>>,
    mut last: Local<Option<MirrorMemo>>,
) {
    let Some(mut bus) = bus else {
        return;
    };
    let now_summary = MirrorMemo::from(&*state);
    let prior_phase_label = last.as_ref().and_then(|m| m.phase_label);

    match &*state {
        MslLoadState::NotStarted => {}
        MslLoadState::Loading { phase, bytes_done, bytes_total } => {
            let label = msl_phase_label(*phase);
            // Phase transition → discrete history entry.
            if prior_phase_label != Some(label) {
                bus.push(MSL_SOURCE, StatusLevel::Info, label);
            }
            // Progress tick (in-place; doesn't accumulate in history).
            let detail = format_progress_detail(*phase, *bytes_done, *bytes_total);
            bus.push_progress(MSL_SOURCE, detail, *bytes_done, *bytes_total);
        }
        MslLoadState::Ready { file_count, .. } => {
            // Only fire once per Ready transition (re-renders shouldn't spam).
            if !matches!(last.as_ref(), Some(MirrorMemo { ready: true, .. })) {
                bus.push(MSL_SOURCE, StatusLevel::Info, format!("ready — {file_count} files"));
                bus.clear_progress(MSL_SOURCE);
            }
        }
        MslLoadState::Failed(msg) => {
            if !matches!(last.as_ref(), Some(MirrorMemo { failed: true, .. })) {
                bus.push(MSL_SOURCE, StatusLevel::Error, msg.clone());
                bus.clear_progress(MSL_SOURCE);
            }
        }
    }

    *last = Some(now_summary);
}

fn msl_phase_label(p: MslLoadPhase) -> &'static str {
    match p {
        MslLoadPhase::FetchingManifest => "fetching manifest",
        MslLoadPhase::FetchingBundle => "downloading",
        MslLoadPhase::LoadingCache => "loading from cache",
        MslLoadPhase::Decompressing => "decompressing",
        MslLoadPhase::Parsing => "loading",
    }
}

fn format_progress_detail(phase: MslLoadPhase, done: u64, total: u64) -> String {
    let label = msl_phase_label(phase);
    match phase {
        MslLoadPhase::Parsing if total > 0 => format!("{label} {done} / {total}"),
        _ if total > 0 => format!(
            "{label} — {:.1} / {:.1} MB",
            done as f64 / 1_048_576.0,
            total as f64 / 1_048_576.0,
        ),
        _ => label.to_string(),
    }
}

/// One-frame memo so the mirror only emits discrete history entries on actual
/// transitions (not on every re-render of the same state).
#[derive(Default)]
pub struct MirrorMemo {
    phase_label: Option<&'static str>,
    ready: bool,
    failed: bool,
}

impl From<&MslLoadState> for MirrorMemo {
    fn from(s: &MslLoadState) -> Self {
        match s {
            MslLoadState::NotStarted => Self::default(),
            MslLoadState::Loading { phase, .. } => Self {
                phase_label: Some(msl_phase_label(*phase)),
                ..Self::default()
            },
            MslLoadState::Ready { .. } => Self { ready: true, ..Self::default() },
            MslLoadState::Failed(_) => Self { failed: true, ..Self::default() },
        }
    }
}

/// Drain core live-sim sample batches ([`crate::SimSampleStream`]) into the viz
/// `SignalRegistry` — the reactive UI projection of the running simulation.
///
/// This is the plot-aware half of the old `worker::handle_modelica_responses`
/// viz block; the core handler now only appends UI-agnostic samples. Per batch:
/// clear history on a fresh compile, push every scalar, attach doc-index
/// descriptions on compile/param-update, and reset the default graph bindings.
pub fn drain_sim_samples_to_viz(
    mut stream: ResMut<crate::SimSampleStream>,
    mut signals: Option<ResMut<SignalRegistry>>,
    mut viz_registry: Option<ResMut<VisualizationRegistry>>,
    doc_registry: Option<Res<crate::state::ModelicaDocumentRegistry>>,
) {
    if stream.batches.is_empty() {
        return;
    }
    // Always take (so the queue can't grow); drop if there's no SignalRegistry.
    let batches = std::mem::take(&mut stream.batches);
    let Some(sigs) = signals.as_deref_mut() else {
        return;
    };
    for batch in &batches {
        if batch.is_new_model {
            for (name, _) in &batch.samples {
                sigs.clear_history(&SignalRef::new(batch.entity, name.clone()));
            }
        }
        for (name, val) in &batch.samples {
            sigs.push_scalar(SignalRef::new(batch.entity, name.clone()), batch.time, *val);
        }
        // Descriptions from the document index (canonical AST projection),
        // looked up by leaf name — refreshed on compile-type results.
        if batch.is_new_model || batch.is_parameter_update {
            let index_ref = doc_registry
                .as_deref()
                .and_then(|r| r.host(batch.document))
                .map(|h| h.document().index());
            if let Some(index) = index_ref {
                for (name, _) in &batch.samples {
                    let Some(entry) = index.find_component_by_leaf(name) else {
                        continue;
                    };
                    if entry.description.is_empty() {
                        continue;
                    }
                    sigs.update_meta(
                        SignalRef::new(batch.entity, name.clone()),
                        SignalMeta {
                            description: Some(entry.description.clone()),
                            unit: None,
                            provenance: Some("modelica".to_string()),
                        },
                    );
                }
            }
        }
        // A fresh compile starts the default plot empty (users add signals via
        // the Telemetry panel) — clear any stale bindings from a prior model.
        if batch.is_new_model {
            if let Some(reg) = viz_registry.as_deref_mut() {
                if let Some(cfg) = reg.get_mut(crate::ui::viz::DEFAULT_MODELICA_GRAPH) {
                    cfg.inputs.clear();
                }
            }
        }
    }
}

/// Reactive UI: project core [`crate::ModelicaNotice`] events into the Console
/// panel. The core worker emits notices; this observer renders them.
pub fn drain_notices_to_console(
    mut notices: MessageReader<crate::ModelicaNotice>,
    console: Option<ResMut<crate::ui::panels::console::ConsoleLog>>,
) {
    let Some(mut console) = console else {
        notices.clear();
        return;
    };
    for n in notices.read() {
        match n.level {
            crate::NoticeLevel::Info => console.info(n.text.clone()),
            crate::NoticeLevel::Warn => console.warn(n.text.clone()),
            crate::NoticeLevel::Error => console.error(n.text.clone()),
        }
    }
}

/// Reactive UI: project core `SourceRootRegistry` load-state transitions into
/// the status bar — progress while `Loading`, a completion entry on
/// `Ready`/`Failed`. Core sets the registry state; it no longer touches the bus.
pub fn mirror_source_roots_to_status_bus(
    registry: Option<Res<crate::source_roots::SourceRootRegistry>>,
    bus: Option<ResMut<StatusBus>>,
    mut last: Local<std::collections::HashMap<String, u8>>,
) {
    use crate::source_roots::{LoadState, STATUS_BUS_SOURCE};
    let (Some(registry), Some(mut bus)) = (registry, bus) else {
        return;
    };
    for (id, root) in &registry.roots {
        let disc = match &root.state {
            LoadState::NotLoaded => 0u8,
            LoadState::Loading { .. } => 1,
            LoadState::Ready => 2,
            LoadState::Failed(_) => 3,
        };
        if last.get(id) == Some(&disc) {
            continue;
        }
        match &root.state {
            LoadState::NotLoaded => {}
            LoadState::Loading { .. } => {
                bus.push_progress(STATUS_BUS_SOURCE, format!("Loading library `{id}`…"), 0, 0);
                bus.push(STATUS_BUS_SOURCE, StatusLevel::Info, format!("Loading library `{id}`"));
            }
            LoadState::Ready => {
                bus.clear_progress(STATUS_BUS_SOURCE);
                bus.push(STATUS_BUS_SOURCE, StatusLevel::Info, format!("Library `{id}` ready"));
            }
            LoadState::Failed(msg) => {
                bus.clear_progress(STATUS_BUS_SOURCE);
                bus.push(
                    STATUS_BUS_SOURCE,
                    StatusLevel::Warn,
                    format!("Library `{id}` load failed: {msg}"),
                );
            }
        }
        last.insert(id.clone(), disc);
    }
}

/// Reactive UI: translate core [`crate::CompileRequested`] events into the UI
/// `CompileModel` command. The core stepper asks for a compile without ever
/// naming the UI command type.
pub fn relay_compile_requests(
    mut requests: MessageReader<crate::CompileRequested>,
    mut commands: Commands,
) {
    for r in requests.read() {
        commands.trigger(crate::ui::commands::CompileModel {
            doc: r.doc,
            class: r.class.clone(),
            force: r.force,
            resume_after_compile: r.resume_after_compile,
        });
    }
}

/// Feed UI input/workspace state into the core [`crate::engine_resource::ParsePacing`]
/// hints that `drive_engine_sync` reads. The core parse scheduler consumes the
/// hints (typing debounce, active-tab priority) without ever naming the UI
/// resources. Ordered `.before(drive_engine_sync)` so the hints are fresh for
/// this frame's parse decisions.
pub fn feed_parse_pacing(
    mut pacing: ResMut<crate::engine_resource::ParsePacing>,
    activity: Res<crate::ui::input_activity::InputActivity>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
) {
    pacing.input_active = activity.is_active();
    pacing.active_document = workspace.as_deref().and_then(|ws| ws.active_document);
}

/// Reactive UI: project terminal experiment-run events into the UI surfaces.
/// The core `drain_pending_handles` writes results/status into the registry and
/// emits the lifecycle messages; this observer renders them — console lines for
/// every terminal state, plus (on completion) the plot auto-pick and the
/// `SignalRegistry` playback publish that canvas plot tiles resolve against.
///
/// All result data is recovered from the registry (core wrote it before the
/// message fired), so the messages stay thin and core never touches the plot /
/// signal / console resources.
pub fn project_run_results_to_ui(
    mut commands: Commands,
    mut ev_completed: MessageReader<lunco_experiments::RunCompleted>,
    mut ev_failed: MessageReader<lunco_experiments::RunFailed>,
    mut ev_cancelled: MessageReader<lunco_experiments::RunCancelled>,
    registry: Res<lunco_experiments::ExperimentRegistry>,
    sources: Res<crate::experiments_runner::ExperimentSources>,
    mut playback: ResMut<crate::experiments_runner::PlaybackEntities>,
    mut console: Option<ResMut<crate::ui::panels::console::ConsoleLog>>,
    mut plot_states: Option<ResMut<crate::ui::panels::experiments::PlotPanelStates>>,
    active_plot: Option<Res<crate::ui::panels::experiments::ActivePlot>>,
    mut signals: Option<ResMut<SignalRegistry>>,
) {
    for ev in ev_completed.read() {
        let run_id = ev.experiment_id;
        let Some(entry) = registry.get(run_id) else { continue };
        let run_name = entry.name.clone();
        let Some(result) = entry.result.as_ref() else { continue };
        let n_samples = result.times.len();
        let n_vars = result.series.len();
        let wall = result.meta.wall_time_ms;

        // Auto-visible: a run that just completed is what the user is looking
        // at, no checkbox needed. Mark it visible on the active plot tab only
        // (per-plot visibility — other plot windows stay untouched, matching
        // Dymola's per-window curve set). Also auto-pick a few variables on the
        // very first completion so the plot has content without hunting through
        // Telemetry. Skip parameters (constant series) — pick the first 3
        // dynamic signals by series-variance heuristic.
        if let Some(states) = plot_states.as_mut() {
            let viz = active_plot
                .as_deref()
                .copied()
                .unwrap_or_default()
                .or_default();
            let entry = states.entry(viz);
            entry.visible_experiments.insert(run_id);
            if entry.picked_vars.is_empty() {
                let mut by_var: Vec<(&String, f64)> = result
                    .series
                    .iter()
                    .map(|(k, v)| {
                        let n = v.len().max(1) as f64;
                        let mean = v.iter().copied().sum::<f64>() / n;
                        let var = v.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n;
                        (k, var)
                    })
                    .filter(|(_, v)| v.is_finite() && *v > 1e-12)
                    .collect();
                by_var.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                for (k, _) in by_var.into_iter().take(3) {
                    entry.picked_vars.insert(k.clone());
                }
            }
        }

        if let Some(c) = console.as_mut() {
            c.info(format!(
                "✓ {run_name} done: {n_samples} samples × {n_vars} vars in {wall} ms"
            ));
        }

        // Publish the run's series into `SignalRegistry` under a per-doc
        // playback entity, so canvas plot tiles bound by `PlotBinding::Doc`
        // resolve to real (entity, path) samples without needing a live cosim
        // entity. One entity per doc, reused across runs — drop prior signals
        // then push the new run's data.
        if let (Some(doc_id), Some(signals_mut)) =
            (sources.0.get(&run_id).copied(), signals.as_deref_mut())
        {
            let entity = *playback
                .0
                .entry(doc_id)
                .or_insert_with(|| commands.spawn_empty().id());
            signals_mut.drop_entity(entity);
            for (path, samples) in &result.series {
                let sig = SignalRef { entity, path: path.clone() };
                for (t, v) in result.times.iter().zip(samples.iter()) {
                    signals_mut.push_scalar(sig.clone(), *t, *v);
                }
            }
        }
    }

    for ev in ev_failed.read() {
        let run_name = registry
            .get(ev.experiment_id)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| "Fast Run".into());
        if let Some(c) = console.as_mut() {
            c.error(format!("⚠ {run_name} FAILED: {}", ev.error));
        }
    }

    for ev in ev_cancelled.read() {
        let run_name = registry
            .get(ev.experiment_id)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| "Fast Run".into());
        if let Some(c) = console.as_mut() {
            c.info(format!("⊘ {run_name} cancelled"));
        }
    }
}
