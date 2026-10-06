//! UI-reactive observers of CORE state.
//!
//! These systems are the *reactive UI layer built on top of the core*: they
//! read core domain state (e.g. [`lunco_modelica_library::source_library::LibraryLoadState`]) and
//! project it into UI surfaces (the workbench status bus, console, plots).
//! The core never references these surfaces — it just owns the observable
//! state. All of this is `ui`-feature only; a headless build has no observers
//! and therefore no egui/workbench dependency.

use bevy::prelude::*;
use lunco_status_core::status_bus::{StatusBus, StatusLevel};
use lunco_telemetry::TelemetrySettings;
use lunco_ui::log::LogBuffer;
use lunco_viz::{SignalMeta, SignalRef, SignalRegistry, VisualizationRegistry};

use lunco_assets_runtime::library::{LibraryLoadPhase, LibraryLoadState};

const SOURCE_LIBRARY_STATUS: &str = "source-library";

/// Watch [`LibraryLoadState`] and translate transitions / progress ticks into
/// [`StatusBus`] events. Phase changes become discrete `Info` entries
/// (preserved in history); byte/file counts within a phase become `Progress`
/// ticks (updated in place).
///
/// This is a pure state mirror, not a task owner — `LibraryLoadState` itself is the
/// lifetime authority, so it uses the status bus's global state-projection API.
pub fn mirror_library_state_to_status_bus(
    state: Res<LibraryLoadState>,
    bus: Option<ResMut<StatusBus>>,
    mut last: Local<Option<MirrorMemo>>,
) {
    let Some(mut bus) = bus else {
        return;
    };
    let now_summary = MirrorMemo::from(&*state);
    let prior_phase_label = last.as_ref().and_then(|m| m.phase_label);

    match &*state {
        LibraryLoadState::NotStarted => {}
        LibraryLoadState::Loading {
            phase,
            bytes_done,
            bytes_total,
        } => {
            let label = library_phase_label(*phase);
            // Phase transition → discrete history entry.
            if prior_phase_label != Some(label) {
                bus.push(SOURCE_LIBRARY_STATUS, StatusLevel::Info, label);
            }
            // Progress tick (in-place; doesn't accumulate in history).
            let detail = format_progress_detail(*phase, *bytes_done, *bytes_total);
            bus.set_progress(SOURCE_LIBRARY_STATUS, detail, *bytes_done, *bytes_total);
        }
        LibraryLoadState::Ready { file_count, .. } => {
            // Only fire once per Ready transition (re-renders shouldn't spam).
            if !matches!(last.as_ref(), Some(MirrorMemo { ready: true, .. })) {
                bus.push(
                    SOURCE_LIBRARY_STATUS,
                    StatusLevel::Info,
                    format!("ready — {file_count} files"),
                );
                bus.remove_progress(SOURCE_LIBRARY_STATUS);
            }
        }
        LibraryLoadState::Failed(msg) => {
            if !matches!(last.as_ref(), Some(MirrorMemo { failed: true, .. })) {
                bus.push(SOURCE_LIBRARY_STATUS, StatusLevel::Error, msg.clone());
                bus.remove_progress(SOURCE_LIBRARY_STATUS);
            }
        }
    }

    *last = Some(now_summary);
}

fn library_phase_label(p: LibraryLoadPhase) -> &'static str {
    match p {
        LibraryLoadPhase::FetchingManifest => "fetching manifest",
        LibraryLoadPhase::FetchingBundle => "downloading",
        LibraryLoadPhase::LoadingCache => "loading from cache",
        LibraryLoadPhase::Decompressing => "decompressing",
        LibraryLoadPhase::Parsing => "loading",
    }
}

fn format_progress_detail(phase: LibraryLoadPhase, done: u64, total: u64) -> String {
    let label = library_phase_label(phase);
    match phase {
        LibraryLoadPhase::Parsing if total > 0 => format!("{label} {done} / {total}"),
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

impl From<&LibraryLoadState> for MirrorMemo {
    fn from(s: &LibraryLoadState) -> Self {
        match s {
            LibraryLoadState::NotStarted => Self::default(),
            LibraryLoadState::Loading { phase, .. } => Self {
                phase_label: Some(library_phase_label(*phase)),
                ..Self::default()
            },
            LibraryLoadState::Ready { .. } => Self {
                ready: true,
                ..Self::default()
            },
            LibraryLoadState::Failed(_) => Self {
                failed: true,
                ..Self::default()
            },
        }
    }
}

/// Drain live-sim sample batches ([`lunco_modelica_runtime::SimSampleStream`]) into the viz
/// `SignalRegistry` — the reactive UI projection of the running simulation.
///
/// This is the plot-aware half of the old `worker::handle_modelica_responses`
/// viz block; the core handler now only appends UI-agnostic samples. Per batch:
/// clear history on a fresh compile, push every scalar, attach doc-index
/// descriptions on compile/param-update, and reset the default graph bindings.
pub fn drain_sim_samples_to_viz(
    mut stream: ResMut<lunco_modelica_runtime::SimSampleStream>,
    mut signals: Option<ResMut<SignalRegistry>>,
    mut viz_registry: Option<ResMut<VisualizationRegistry>>,
    doc_registry: Option<Res<crate::ui::document_context::ModelicaDocuments>>,
    telemetry_settings: Option<Res<TelemetrySettings>>,
    owners: Query<Option<&lunco_core::GlobalEntityId>>,
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
        let recorded: std::collections::HashSet<String> = viz_registry
            .as_deref()
            .into_iter()
            .flat_map(|registry| registry.iter())
            .flat_map(|(_, config)| config.inputs.iter())
            .filter(|binding| binding.source.entity == batch.entity)
            .map(|binding| binding.source.path.clone())
            .collect();
        let deadband = telemetry_settings
            .as_deref()
            .map(|settings| settings.default_deadband)
            .unwrap_or_default();
        if batch.is_new_model {
            for (name, _) in &batch.samples {
                // Core runtime telemetry owns unbound state retention.  Only
                // clear a signal here when this UI-created plot binding is
                // also being refreshed; otherwise a late UI observer would
                // erase the inspector's retained state.
                if recorded.contains(name) {
                    sigs.clear_history(&SignalRef::new(batch.entity, name.clone()));
                }
            }
        }
        for (name, val) in &batch.samples {
            if !recorded.contains(name) {
                continue;
            }
            let signal = SignalRef::new(batch.entity, name.clone());
            if let Ok(Some(owner)) = owners.get(batch.entity) {
                sigs.associate_global_owner(&signal, *owner);
            }
            let changed = sigs
                .scalar_history(&signal)
                .and_then(lunco_signal::ScalarHistory::back)
                .is_none_or(|sample| deadband.changed(sample.value, *val));
            if changed {
                sigs.push_scalar(signal, batch.time, *val);
            }
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
                    if !recorded.contains(name) {
                        continue;
                    }
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
                            group_path: None,
                            exposure: Default::default(),
                            ..Default::default()
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

    // Auto-binding all documented signals to the default graph on catalog change
    // was removed: it caused every telemetry/cosim/modelica signal to appear in
    // the graph on startup, making it unreadable. Users now add signals manually
    // via the Telemetry panel checkboxes.
}

/// Publish lifecycle notices to Recent status; the shared history adapter feeds Console.
pub fn drain_notices_to_status_bus(
    mut notices: MessageReader<lunco_modelica_runtime::ModelicaNotice>,
    mut bus: ResMut<StatusBus>,
) {
    for n in notices.read() {
        let level = match n.level {
            lunco_modelica_runtime::NoticeLevel::Info => StatusLevel::Info,
            lunco_modelica_runtime::NoticeLevel::Warn => StatusLevel::Warn,
            lunco_modelica_runtime::NoticeLevel::Error => StatusLevel::Error,
        };
        bus.push("Modelica", level, n.text.clone());
    }
}

/// Reactive UI: project source-root load-state transitions into the status bar
/// — progress while `Loading`, a completion entry on `Ready`/`Failed`. The
/// source-root host sets the registry state; this adapter only presents it.
pub fn mirror_source_roots_to_status_bus(
    registry: Option<Res<lunco_modelica_source_roots::SourceRootRegistry>>,
    bus: Option<ResMut<StatusBus>>,
    mut last: Local<std::collections::HashMap<String, u8>>,
) {
    use lunco_modelica_source_roots::{LoadState, STATUS_BUS_SOURCE};
    let (Some(registry), Some(mut bus)) = (registry, bus) else {
        return;
    };
    let present = registry
        .roots
        .keys()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    last.retain(|id, _| present.contains(id));
    let mut roots = registry.roots.iter().collect::<Vec<_>>();
    roots.sort_unstable_by(|left, right| left.0.cmp(right.0));
    let mut loading = Vec::new();
    for (id, root) in roots {
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
                loading.push(id.as_str());
                bus.push(
                    STATUS_BUS_SOURCE,
                    StatusLevel::Info,
                    format!("Loading library `{id}`"),
                );
            }
            LoadState::Ready => {
                bus.push(
                    STATUS_BUS_SOURCE,
                    StatusLevel::Info,
                    format!("Library `{id}` ready"),
                );
            }
            LoadState::Failed(msg) => {
                bus.push(
                    STATUS_BUS_SOURCE,
                    StatusLevel::Warn,
                    format!("Library `{id}` load failed: {msg}"),
                );
            }
        }
        last.insert(id.clone(), disc);
    }
    if loading.is_empty() {
        bus.remove_progress(STATUS_BUS_SOURCE);
    } else {
        bus.set_progress(
            STATUS_BUS_SOURCE,
            format!("Loading Modelica source roots: {}", loading.join(", ")),
            0,
            loading.len() as u64,
        );
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
    sources: Res<lunco_experiments::ExperimentOrigins>,
    mut playback: ResMut<lunco_modelica_runner::PlaybackEntities>,
    mut console: Option<ResMut<LogBuffer>>,
    mut plot_states: Option<ResMut<lunco_experiments_ui::PlotPanelStates>>,
    active_plot: Option<Res<lunco_experiments_ui::ActivePlot>>,
    mut signals: Option<ResMut<SignalRegistry>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
    pins: Option<Res<crate::ui::doc_pin::DocPinState>>,
) {
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    let workspace = workspace.as_deref().map(|workspace| &workspace.0);
    let selected_document = pins
        .as_deref()
        .and_then(|pins| pins.experiments)
        .or_else(|| workspace.and_then(|workspace| workspace.active_document));
    for ev in ev_completed.read() {
        let run_id = ev.experiment_id;
        let Some(entry) = registry.get(run_id) else {
            continue;
        };
        if !matches!(entry.status, lunco_experiments::RunStatus::Done { .. }) {
            continue;
        }
        let Some(source) = ev.origin.local_document() else {
            lunco_core::trigger_runtime_error(
                &mut commands,
                "experiment-ui-publication-failed",
                format!("completed experiment {run_id:?} has no source attribution"),
            );
            continue;
        };
        if Some(&ev.origin) != sources.get(&run_id)
            || !source.is_in_active_scope(workspace, replication.as_ref())
        {
            continue;
        }
        let run_name = entry.name.clone();
        let Some(result) = entry.result.as_ref() else {
            continue;
        };
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
        if let Some(states) = plot_states.as_mut()
            && selected_document.is_none_or(|doc| doc == source.document)
        {
            let viz = active_plot
                .as_deref()
                .copied()
                .unwrap_or_default()
                .or_default(crate::ui::viz::DEFAULT_MODELICA_GRAPH);
            states.sync_twin(viz, &crate::ui::doc_pin::twin_id_for_doc(source.document));
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
                "{run_name} done: {n_samples} samples x {n_vars} vars in {wall} ms"
            ));
        }

        // Publish the run's series into `SignalRegistry` under a per-doc
        // playback entity, so canvas plot tiles bound by `PlotBinding::Doc`
        // resolve to real (entity, path) samples without needing a live cosim
        // entity. One entity per doc, reused across runs — drop prior signals
        // then push the new run's data.
        if let Some(signals_mut) = signals.as_deref_mut() {
            let doc_id = source.document;
            let entity = *playback
                .0
                .entry(doc_id)
                .or_insert_with(|| commands.spawn_empty().id());
            commands.entity(entity).try_insert(source.clone());
            signals_mut.drop_entity(entity);
            for (path, samples) in &result.series {
                let sig = SignalRef {
                    entity,
                    path: path.clone(),
                };
                for (t, v) in result.times.iter().zip(samples.iter()) {
                    signals_mut.push_scalar(sig.clone(), *t, *v);
                }
            }
        }
    }

    for ev in ev_failed.read() {
        let Some(entry) = registry.get(ev.experiment_id) else {
            continue;
        };
        if Some(&ev.origin) != sources.get(&ev.experiment_id)
            || !ev
                .origin
                .local_document()
                .is_some_and(|source| source.is_in_active_scope(workspace, replication.as_ref()))
            || !matches!(entry.status, lunco_experiments::RunStatus::Failed { .. })
        {
            continue;
        }
        let run_name = &entry.name;
        if let Some(c) = console.as_mut() {
            c.error(format!("{run_name} FAILED: {}", ev.error));
        }
    }

    for ev in ev_cancelled.read() {
        let Some(entry) = registry.get(ev.experiment_id) else {
            continue;
        };
        if Some(&ev.origin) != sources.get(&ev.experiment_id)
            || !ev
                .origin
                .local_document()
                .is_some_and(|source| source.is_in_active_scope(workspace, replication.as_ref()))
            || !matches!(entry.status, lunco_experiments::RunStatus::Cancelled)
        {
            continue;
        }
        let run_name = &entry.name;
        if let Some(c) = console.as_mut() {
            c.info(format!("{run_name} cancelled"));
        }
    }
}

/// Retire active playback while completed trajectories remain in run history.
pub fn retire_closed_twin_playback(
    trigger: On<lunco_workspace::TwinClosed>,
    mut commands: Commands,
) {
    queue_retire_runtime_projection(
        &mut commands,
        lunco_workspace::DocumentRuntimeOwner::LocalTwin(trigger.event().twin),
    );
}

pub fn retire_replication_playback(
    trigger: On<lunco_core_session::ReplicationOwnerRetired>,
    mut commands: Commands,
) {
    queue_retire_runtime_projection(
        &mut commands,
        lunco_workspace::DocumentRuntimeOwner::Replicated(trigger.event().owner.clone()),
    );
}

fn queue_retire_runtime_projection(
    commands: &mut Commands,
    owner: lunco_workspace::DocumentRuntimeOwner,
) {
    // Apply after earlier playback spawn/attribution commands have committed.
    commands.queue(move |world: &mut World| {
        let retired: Vec<_> = world
            .resource::<lunco_modelica_runner::PlaybackEntities>()
            .0
            .iter()
            .filter_map(|(document, entity)| {
                world
                    .get::<lunco_workspace::PinnedDocumentRuntimeOwner>(*entity)
                    .is_some_and(|source| source.runtime == owner)
                    .then_some((*document, *entity))
            })
            .collect();
        let mut closed_documents: std::collections::HashSet<_> =
            retired.iter().map(|(doc, _)| *doc).collect();
        if let Some(workspace) = world.get_resource::<lunco_workspace::WorkspaceResource>() {
            closed_documents.extend(
                workspace
                    .documents()
                    .iter()
                    .filter(|entry| entry.runtime_context == owner)
                    .map(|entry| entry.id),
            );
        }
        if let Some(sources) = world.get_resource::<lunco_experiments::ExperimentOrigins>() {
            closed_documents.extend(
                sources
                    .iter()
                    .filter_map(|(_, origin)| origin.local_document())
                    .filter(|source| source.runtime == owner)
                    .map(|source| source.document),
            );
        }
        let mut cancelled_duplicates = Vec::new();
        if let Some(mut openings) =
            world.get_resource_mut::<crate::ui::document_openings::DocumentOpenings>()
        {
            for document in &closed_documents {
                if matches!(
                    openings.cancel(*document),
                    Some(crate::ui::document_openings::OpeningState::Duplicate(_))
                ) {
                    cancelled_duplicates.push(*document);
                }
            }
        }
        for document in cancelled_duplicates {
            crate::ui::panels::canvas_diagram::loads::retire_duplicate_placeholder_in(
                world, document,
            );
        }
        if let Some(mut pins) = world.get_resource_mut::<crate::ui::doc_pin::DocPinState>() {
            for document in &closed_documents {
                pins.forget(*document);
            }
        }
        let retired_runs: Vec<_> = world
            .get_resource::<lunco_experiments::ExperimentOrigins>()
            .map(|origins| {
                origins
                    .iter()
                    .filter(|(_, origin)| origin.belongs_to_runtime(&owner))
                    .map(|(id, _)| *id)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(mut states) = world.get_resource_mut::<lunco_experiments_ui::PlotPanelStates>()
        {
            for run in retired_runs {
                states.forget_experiment(run);
            }
            for document in &closed_documents {
                states.forget_scope(&crate::ui::doc_pin::twin_id_for_doc(*document));
            }
        }
        for (document, entity) in retired {
            world
                .resource_mut::<lunco_modelica_runner::PlaybackEntities>()
                .0
                .remove(&document);
            if let Some(mut signals) = world.get_resource_mut::<SignalRegistry>() {
                signals.drop_entity(entity);
            }
            world.despawn(entity);
        }
    });
}

pub fn forget_removed_plot_runs(
    mut removed: MessageReader<lunco_experiments::ExperimentRemoved>,
    mut states: Option<ResMut<lunco_experiments_ui::PlotPanelStates>>,
) {
    for event in removed.read() {
        if let Some(states) = states.as_deref_mut() {
            states.forget_experiment(event.experiment_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_twin_retires_only_its_playback_projection() {
        use lunco_modelica_runner::PlaybackEntities;
        use lunco_workspace::PinnedDocumentRuntimeOwner;
        let mut app = App::new();
        app.init_resource::<PlaybackEntities>()
            .init_resource::<SignalRegistry>()
            .init_resource::<crate::ui::doc_pin::DocPinState>()
            .init_resource::<lunco_experiments_ui::PlotPanelStates>()
            .add_observer(retire_closed_twin_playback);
        let twin = lunco_workspace::TwinId::new(1);
        let mut entities = Vec::new();
        for (index, runtime_twin) in [Some(twin), Some(lunco_workspace::TwinId::new(2)), None]
            .into_iter()
            .enumerate()
        {
            let document = lunco_doc::DocumentId::new(index as u64 + 1);
            let entity = app
                .world_mut()
                .spawn(PinnedDocumentRuntimeOwner {
                    document,
                    runtime: runtime_twin.map_or(
                        lunco_workspace::DocumentRuntimeOwner::Application,
                        lunco_workspace::DocumentRuntimeOwner::LocalTwin,
                    ),
                })
                .id();
            app.world_mut()
                .resource_mut::<PlaybackEntities>()
                .0
                .insert(document, entity);
            app.world_mut()
                .resource_mut::<SignalRegistry>()
                .push_scalar(
                    SignalRef {
                        entity,
                        path: "value".into(),
                    },
                    0.0,
                    1.0,
                );
            entities.push(entity);
        }
        let closed_document = lunco_doc::DocumentId::new(1);
        app.world_mut()
            .resource_mut::<crate::ui::doc_pin::DocPinState>()
            .experiments = Some(closed_document);
        let viz = lunco_viz::VizId(7);
        {
            let mut states = app
                .world_mut()
                .resource_mut::<lunco_experiments_ui::PlotPanelStates>();
            states.sync_twin(
                viz,
                &crate::ui::doc_pin::twin_id_for_doc(lunco_doc::DocumentId::new(2)),
            );
            states.set_var(viz, "other_owner".into(), true);
            states.sync_twin(viz, &crate::ui::doc_pin::twin_id_for_doc(closed_document));
            states.set_var(viz, "closed_owner".into(), true);
        }
        app.world_mut().trigger(lunco_workspace::TwinClosed {
            twin,
            root: Default::default(),
            was_active: true,
        });
        app.world_mut().flush();
        assert!(app.world().get_entity(entities[0]).is_err());
        assert!(app.world().get_entity(entities[1]).is_ok());
        assert!(app.world().get_entity(entities[2]).is_ok());
        assert_eq!(app.world().resource::<PlaybackEntities>().0.len(), 2);
        let signals = app.world().resource::<SignalRegistry>();
        assert_eq!(signals.iter_signals().count(), 2);
        assert!(
            signals
                .iter_signals()
                .all(|(signal, _)| signal.entity != entities[0])
        );
        assert_eq!(
            app.world()
                .resource::<crate::ui::doc_pin::DocPinState>()
                .experiments,
            None
        );
        let mut states = app
            .world_mut()
            .resource_mut::<lunco_experiments_ui::PlotPanelStates>();
        assert!(states.get(viz).is_none());
        states.sync_twin(
            viz,
            &crate::ui::doc_pin::twin_id_for_doc(lunco_doc::DocumentId::new(2)),
        );
        assert!(states.picked(viz).contains("other_owner"));
        states.sync_twin(viz, &crate::ui::doc_pin::twin_id_for_doc(closed_document));
        assert!(states.picked(viz).is_empty());
    }

    #[test]
    fn modelica_lifecycle_notices_reach_recent_status_once() {
        use lunco_modelica_runtime::{ModelicaNotice, NoticeLevel};
        let mut app = App::new();
        app.add_message::<ModelicaNotice>()
            .init_resource::<StatusBus>()
            .add_systems(Update, drain_notices_to_status_bus);
        for (level, text) in [
            (NoticeLevel::Info, "Preparing simulation equations"),
            (NoticeLevel::Warn, "Discarded stale preparation"),
            (NoticeLevel::Error, "Preparation failed"),
        ] {
            app.world_mut().write_message(ModelicaNotice {
                level,
                text: text.into(),
            });
        }
        app.update();
        let levels: Vec<_> = app
            .world()
            .resource::<StatusBus>()
            .history()
            .map(|event| event.level)
            .collect();
        assert_eq!(
            levels,
            vec![StatusLevel::Info, StatusLevel::Warn, StatusLevel::Error]
        );
        assert_eq!(
            app.world()
                .resource::<StatusBus>()
                .active_progress()
                .count(),
            0
        );
        app.update();
        assert_eq!(app.world().resource::<StatusBus>().history().count(), 3);
    }

    #[test]
    fn standalone_modelica_entities_still_project_live_samples() {
        let mut app = App::new();
        app.init_resource::<lunco_modelica_runtime::SimSampleStream>()
            .init_resource::<SignalRegistry>()
            .init_resource::<VisualizationRegistry>()
            .add_systems(Update, drain_sim_samples_to_viz);

        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<VisualizationRegistry>()
            .insert(lunco_viz::VisualizationConfig {
                id: crate::ui::viz::DEFAULT_MODELICA_GRAPH,
                title: "Modelica".into(),
                kind: lunco_viz::LINE_PLOT_KIND,
                view: lunco_viz::ViewTarget::Panel2D,
                inputs: vec![lunco_viz::SignalBinding::live(
                    SignalRef::new(entity, "solar_power"),
                    "y",
                )],
                style: serde_json::Value::Null,
            });
        app.world_mut()
            .resource_mut::<lunco_modelica_runtime::SimSampleStream>()
            .batches
            .push(lunco_modelica_runtime::SimSampleBatch {
                entity,
                document: lunco_doc::DocumentId::default(),
                time: 1.0,
                samples: vec![("solar_power".to_string(), 307.0)],
                is_new_model: false,
                is_parameter_update: false,
            });

        app.update();

        let history = app
            .world()
            .resource::<SignalRegistry>()
            .scalar_history(&SignalRef::new(entity, "solar_power"))
            .expect("standalone Modelica output should still reach viz");
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn unselected_modelica_state_is_not_retained() {
        let mut app = App::new();
        app.init_resource::<lunco_modelica_runtime::SimSampleStream>()
            .init_resource::<SignalRegistry>()
            .add_systems(Update, drain_sim_samples_to_viz);
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<lunco_modelica_runtime::SimSampleStream>()
            .batches
            .push(lunco_modelica_runtime::SimSampleBatch {
                entity,
                document: lunco_doc::DocumentId::default(),
                time: 1.0,
                samples: vec![("solar_power".to_string(), 307.0)],
                is_new_model: false,
                is_parameter_update: false,
            });

        app.update();

        assert!(
            app.world()
                .resource::<SignalRegistry>()
                .scalar_history(&SignalRef::new(entity, "solar_power"))
                .is_none(),
            "the inspector exposes live state; history exists only after recording is selected"
        );
    }
}
