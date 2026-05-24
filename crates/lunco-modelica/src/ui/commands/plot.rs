//! Visualization/Plotting commands: NewPlotPanel and AddSignalToPlot.

use bevy::prelude::*;
use lunco_core::{Command, on_command};

// ─── Command Structs ─────────────────────────────────────────────────────────

#[Command(default)]
pub struct NewPlotPanel {
    pub title: String,
    pub signals: Vec<String>,
    pub source: u64,
}

#[Command(default)]
pub struct AddSignalToPlot {
    pub plot: u64,
    pub signal: String,
}

// ─── Observers ───────────────────────────────────────────────────────────────

#[on_command(NewPlotPanel)]
pub fn on_new_plot_panel(trigger: On<NewPlotPanel>, mut commands: Commands) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        use lunco_viz::{
            kinds::line_plot::LINE_PLOT_KIND, view::ViewTarget, viz::SignalBinding,
            viz::VisualizationConfig, viz::VizId, SignalRef, VisualizationRegistry,
        };
        let id = VizId::next();
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
                    .get_resource::<crate::ui::panels::experiments::PlotPanelStates>()
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
        let model_entity = world
            .query::<(bevy::prelude::Entity, &crate::ModelicaModel)>()
            .iter(world)
            .next()
            .map(|(e, _)| e);
        let mut inputs: Vec<SignalBinding> = cloned_inputs;
        for s in &ev.signals {
            let entity = model_entity.unwrap_or(bevy::prelude::Entity::PLACEHOLDER);
            if inputs.iter().any(|b| b.source.entity == entity && b.source.path == *s) {
                continue;
            }
            inputs.push(SignalBinding {
                source: SignalRef::new(entity, s.clone()),
                role: "y".into(),
                label: None,
                color: None,
                visible: true,
            });
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
        if !cloned_picked.is_empty() {
            if let Some(mut states) = world
                .get_resource_mut::<crate::ui::panels::experiments::PlotPanelStates>()
            {
                states.entry(id).picked_vars = cloned_picked;
            }
        }
        world.commands().trigger(lunco_workbench::OpenTab {
            kind: crate::ui::panels::graphs::MODELICA_PLOT_KIND,
            instance: id.0,
        });
        bevy::log::info!("[NewPlotPanel] opened `{}` (id={})", title, id.0);
    });
}

#[on_command(AddSignalToPlot)]
pub fn on_add_signal_to_plot(trigger: On<AddSignalToPlot>, mut commands: Commands) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        use lunco_viz::{viz::SignalBinding, viz::VizId, SignalRef, VisualizationRegistry};
        let id = if ev.plot == 0 {
            crate::ui::viz::DEFAULT_MODELICA_GRAPH
        } else {
            VizId(ev.plot)
        };
        let model_entity = world
            .query::<(bevy::prelude::Entity, &crate::ModelicaModel)>()
            .iter(world)
            .next()
            .map(|(e, _)| e)
            .unwrap_or(bevy::prelude::Entity::PLACEHOLDER);
        let mut registry = world.resource_mut::<VisualizationRegistry>();
        let Some(cfg) = registry.get_mut(id) else {
            bevy::log::warn!("[AddSignalToPlot] no plot with id={}", ev.plot);
            return;
        };
        let signal_ref = SignalRef::new(model_entity, ev.signal.clone());
        if cfg.inputs.iter().any(|b| b.source == signal_ref) {
            return;
        }
        cfg.inputs.push(SignalBinding {
            source: signal_ref,
            role: "y".into(),
            label: None,
            color: None,
            visible: true,
        });
    });
}
