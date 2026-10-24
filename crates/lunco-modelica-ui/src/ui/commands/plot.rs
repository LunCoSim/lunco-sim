//! Visualization/Plotting commands: NewPlotPanel and AddSignalToPlot.

use bevy::prelude::*;
use lunco_core::{Command, on_command};

// ─── Command Structs ─────────────────────────────────────────────────────────

/// Open a new plot panel. With `source` set it duplicates that plot's signal
/// bindings and picked series — "open another view of this, then diverge" —
/// otherwise it starts from `signals`.
#[Command(default)]
pub struct NewPlotPanel {
    /// Panel title. Empty = derived from `source` (`"<title> (copy)"`), or a
    /// default when there is no source.
    pub title: String,
    /// Signal names to plot initially.
    pub signals: Vec<String>,
    /// Explicit live source. Omitted uses only the active/pinned document's
    /// simulator; a batch-only document adds historical picks without inventing
    /// a live binding to an unrelated scene participant.
    pub target_gid: Option<lunco_core::GlobalEntityId>,
    /// `VizId` of a plot to clone bindings from. `0` = start empty.
    pub source: u64,
}

/// Add one signal to an existing plot panel.
#[Command(default)]
pub struct AddSignalToPlot {
    /// `VizId` of the target plot panel.
    pub plot: u64,
    /// Signal name to add.
    pub signal: String,
    pub target_gid: Option<lunco_core::GlobalEntityId>,
}

/// Replace one plot's exact signal selection. Unlike the additive telemetry
/// action, this also retires earlier live bindings and historical picks, so a
/// presentation cannot silently mix quantities from a previous analysis.
#[Command(default)]
pub struct SetPlotSignals {
    pub plot: u64,
    pub signals: Vec<String>,
    /// Omission uses only the active/pinned document's admitted simulator.
    pub target_gid: Option<lunco_core::GlobalEntityId>,
}

/// Set the existing historical plot cursor, also consumed by DynamicSelect
/// diagrams. This selects recorded samples; it never advances a simulation.
#[Command(default)]
pub struct SetPlotScrubTime {
    /// Zero selects the canonical Modelica graph.
    pub plot: u64,
    /// Seconds in the experiment; omitted clears the cursor (latest sample).
    pub time_s: Option<f64>,
}

#[on_command(SetPlotScrubTime)]
pub fn on_set_plot_scrub_time(
    trigger: On<SetPlotScrubTime>,
    mut commands: Commands,
    active_id: Option<Res<lunco_core::ActiveCommandId>>,
    pending_request: Option<Res<lunco_api::executor::PendingApiRequest>>,
) {
    let request = trigger.event().clone();
    let command_id = active_id.as_ref().and_then(|id| id.get());
    let correlation_id = pending_request
        .as_ref()
        .map(|r| r.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        let result = (|| {
            let id = if request.plot == 0 {
                crate::ui::viz::DEFAULT_MODELICA_GRAPH
            } else {
                lunco_viz::VizId(request.plot)
            };
            if request.time_s.is_some_and(|t| !t.is_finite()) {
                return Err("Plot cursor time must be finite".to_string());
            }
            if world
                .resource::<lunco_viz::VisualizationRegistry>()
                .get(id)
                .is_none()
            {
                return Err(format!("No plot with id={}", id.0));
            }
            world
                .resource_mut::<lunco_experiments_ui::PlotPanelStates>()
                .set_scrub(id, request.time_s);
            world.resource_mut::<lunco_experiments_ui::ActivePlot>().0 = Some(id);
            Ok(lunco_command_contracts::Ack::new(
                lunco_command_contracts::OpId::new(),
            ))
        })();
        lunco_api::executor::finish_command_result(
            world,
            command_id,
            correlation_id,
            result,
            lunco_api_core::ApiErrorCode::CommandRejected,
        );
    });
}

#[on_command(SetPlotSignals)]
pub fn on_set_plot_signals(trigger: On<SetPlotSignals>, mut commands: Commands) {
    let request = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        use lunco_viz::{SignalRef, VisualizationRegistry, VizId, viz::SignalBinding};
        let id = if request.plot == 0 {
            crate::ui::viz::DEFAULT_MODELICA_GRAPH
        } else {
            VizId(request.plot)
        };
        let entity = match signal_owner(world, request.target_gid) {
            Ok(entity) => entity,
            Err(error) => {
                bevy::log::warn!("[SetPlotSignals] {error}");
                return;
            }
        };
        if request.signals.iter().any(|s| s.trim().is_empty()) {
            bevy::log::warn!("[SetPlotSignals] signal names must not be empty");
            return;
        }
        let picked: std::collections::BTreeSet<_> = request.signals.into_iter().collect();
        let mut registry = world.resource_mut::<VisualizationRegistry>();
        // The domain owns its canonical plot even before its first paint.
        // Preparation commands must not require showing an empty editor first.
        if id == crate::ui::viz::DEFAULT_MODELICA_GRAPH {
            crate::ui::viz::ensure_default_modelica_graph(&mut registry);
        }
        let Some(config) = registry.get_mut(id) else {
            bevy::log::warn!("[SetPlotSignals] no plot with id={}", id.0);
            return;
        };
        config.inputs = entity
            .map(|entity| {
                picked
                    .iter()
                    .map(|signal| SignalBinding::live(SignalRef::new(entity, signal.clone()), "y"))
                    .collect()
            })
            .unwrap_or_default();
        let twin = crate::ui::doc_pin::resolved_experiments_doc(world)
            .map(crate::ui::doc_pin::twin_id_for_doc);
        if let Some(mut states) = world.get_resource_mut::<lunco_experiments_ui::PlotPanelStates>()
        {
            if let Some(twin) = twin {
                states.sync_twin(id, &twin);
            }
            let state = states.entry(id);
            state.picked_vars = picked;
            // An explicit empty selection is also intentional.
            state.auto_show_attempted = true;
        }
    });
}

fn signal_owner(
    world: &World,
    target: Option<lunco_core::GlobalEntityId>,
) -> Result<Option<Entity>, String> {
    if let Some(target) = target {
        return world
            .get_resource::<lunco_api::registry::ApiEntityRegistry>()
            .and_then(|registry| registry.resolve(&target))
            .filter(|entity| {
                world
                    .get::<lunco_modelica_runtime::ModelicaModel>(*entity)
                    .is_some()
            })
            .map(Some)
            .ok_or_else(|| format!("live Modelica source {} is unavailable", target.get()));
    }
    Ok(
        crate::ui::doc_pin::resolved_experiments_doc(world).and_then(|doc| {
            world
                .get_resource::<crate::ui::document_context::ModelicaDocuments>()
                .and_then(|registry| registry.simulator_for(doc))
        }),
    )
}

// ─── Observers ───────────────────────────────────────────────────────────────

#[on_command(NewPlotPanel)]
pub fn on_new_plot_panel(trigger: On<NewPlotPanel>, mut commands: Commands) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        use lunco_viz::{
            SignalRef, VisualizationRegistry, VizId,
            kinds::line_plot::LINE_PLOT_KIND,
            view::ViewTarget,
            viz::{SignalBinding, VisualizationConfig},
        };
        let model_entity = match signal_owner(world, ev.target_gid) {
            Ok(entity) => entity,
            Err(error) => {
                bevy::log::warn!("[NewPlotPanel] {error}");
                return;
            }
        };
        let id = world.resource::<VisualizationRegistry>().allocate_id();
        let source_viz = (ev.source != 0).then_some(VizId(ev.source));
        let cloned_inputs: Vec<SignalBinding> = source_viz
            .and_then(|src| {
                world
                    .get_resource::<VisualizationRegistry>()
                    .and_then(|r| r.get(src))
                    .map(|cfg| cfg.inputs.clone())
            })
            .unwrap_or_default();
        let cloned_picked: std::collections::BTreeSet<String> = source_viz
            .map(|src| {
                world
                    .get_resource::<lunco_experiments_ui::PlotPanelStates>()
                    .map(|s| s.picked(src))
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let title = if !ev.title.is_empty() {
            ev.title.clone()
        } else if source_viz.is_some() {
            source_viz
                .and_then(|src| {
                    world
                        .get_resource::<VisualizationRegistry>()
                        .and_then(|r| r.get(src))
                        .map(|cfg| format!("{} (copy)", cfg.title))
                })
                .unwrap_or_else(|| format!("Plot #{}", id.0))
        } else {
            format!("Plot #{}", id.0)
        };
        let mut inputs: Vec<SignalBinding> = cloned_inputs;
        for s in &ev.signals {
            let Some(entity) = model_entity else { continue };
            if inputs
                .iter()
                .any(|b| b.source.entity == entity && b.source.path == *s)
            {
                continue;
            }
            inputs.push(SignalBinding::live(SignalRef::new(entity, s.clone()), "y"));
        }
        let mut registry = world.resource_mut::<VisualizationRegistry>();
        registry.insert(VisualizationConfig {
            id,
            title: title.clone(),
            kind: LINE_PLOT_KIND,
            view: ViewTarget::Panel2D,
            inputs,
            style: serde_json::Value::Null,
        });
        let mut picked = cloned_picked;
        picked.extend(ev.signals.iter().cloned());
        if !picked.is_empty() {
            let twin = crate::ui::doc_pin::resolved_experiments_doc(world)
                .map(crate::ui::doc_pin::twin_id_for_doc);
            if let Some(mut states) =
                world.get_resource_mut::<lunco_experiments_ui::PlotPanelStates>()
            {
                if let Some(twin) = twin {
                    states.sync_twin(id, &twin);
                }
                states.entry(id).picked_vars = picked;
            }
        }
        let restore = world
            .get_resource::<lunco_experiments_ui::ActivePlot>()
            .and_then(|active| active.0)
            .map(|active| lunco_workbench_core::TabId::Instance {
                kind: crate::ui::panels::graphs::MODELICA_PLOT_KIND,
                instance: active.0,
            });
        world
            .commands()
            .trigger(lunco_workbench_core::commands::OpenTabPreserveFocus {
                kind: crate::ui::panels::graphs::MODELICA_PLOT_KIND,
                instance: id.0,
                restore,
            });
        bevy::log::info!("[NewPlotPanel] opened `{}` (id={})", title, id.0);
    });
}

#[on_command(AddSignalToPlot)]
pub fn on_add_signal_to_plot(trigger: On<AddSignalToPlot>, mut commands: Commands) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        use lunco_viz::{SignalRef, VisualizationRegistry, VizId, viz::SignalBinding};
        let id = if ev.plot == 0 {
            crate::ui::viz::DEFAULT_MODELICA_GRAPH
        } else {
            VizId(ev.plot)
        };
        let model_entity = match signal_owner(world, ev.target_gid) {
            Ok(entity) => entity,
            Err(error) => {
                bevy::log::warn!("[AddSignalToPlot] {error}");
                return;
            }
        };
        let mut registry = world.resource_mut::<VisualizationRegistry>();
        let Some(cfg) = registry.get_mut(id) else {
            bevy::log::warn!("[AddSignalToPlot] no plot with id={}", ev.plot);
            return;
        };
        if let Some(model_entity) = model_entity {
            let signal_ref = SignalRef::new(model_entity, ev.signal.clone());
            if !cfg.inputs.iter().any(|b| b.source == signal_ref) {
                cfg.inputs.push(SignalBinding::live(signal_ref, "y"));
            }
        }
        let twin = crate::ui::doc_pin::resolved_experiments_doc(world)
            .map(crate::ui::doc_pin::twin_id_for_doc);
        if let Some(mut states) = world.get_resource_mut::<lunco_experiments_ui::PlotPanelStates>()
        {
            if let Some(twin) = twin {
                states.sync_twin(id, &twin);
            }
            states.entry(id).picked_vars.insert(ev.signal.clone());
        }
    });
}
