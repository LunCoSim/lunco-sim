//! Render-independent state shared by experiment-capable UI adapters.
//!
//! The experiment registry and run execution live in [`lunco_experiments`].
//! This package owns only the view state that multiple UI surfaces share:
//! per-plot selections, the active plot, and the change-gated sample cache.
//! It deliberately does not know about Modelica documents, egui widgets, or
//! a particular workbench.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use bevy::prelude::{Resource, World};
use lunco_experiments::{ExperimentId, ExperimentRegistry, RunStatus, TwinId};
use lunco_viz_core::VizId;

/// UI state that is shared between the experiment list and plot-related
/// surfaces.
#[derive(Resource, Default, Debug)]
pub struct ExperimentVisibility {
    /// Free-text filter for a variable picker.
    pub var_filter: String,
    /// Inline experiment rename state: run id and current draft text.
    pub editing_name: Option<(ExperimentId, String)>,
    /// Plot receiving run/variable toggles. `None` follows [`ActivePlot`].
    pub target_plot: Option<VizId>,
}

/// State belonging to one plot instance.
#[derive(Default, Debug, Clone)]
pub struct PlotPanelState {
    /// Variables selected in this plot.
    pub picked_vars: BTreeSet<String>,
    /// Optional scrub time.
    pub scrub_time: Option<f64>,
    /// Runs shown in this plot.
    pub visible_experiments: HashSet<ExperimentId>,
    /// Twin whose selections are currently live.
    pub last_twin: Option<TwinId>,
    /// Whether the first-run auto-selection decision has been made.
    pub auto_show_attempted: bool,
    /// Runs already promoted automatically for this plot.
    pub auto_shown: HashSet<ExperimentId>,
    /// Whether this plot uses a log10 Y axis.
    pub log_y: bool,
    /// Whether the user has explicitly changed [`Self::log_y`].
    pub log_y_user_set: bool,
    /// Whether colors identify runs instead of variables.
    pub color_by_run: bool,
}

/// Per-plot state keyed by the visualization instance and archived by Twin.
#[derive(Resource, Default, Debug)]
pub struct PlotPanelStates {
    /// Live state for each plot instance.
    pub by_viz: HashMap<VizId, PlotPanelState>,
    archived: HashMap<(VizId, TwinId), PlotPanelState>,
}

impl PlotPanelStates {
    /// Read the state for a plot without creating it.
    pub fn get(&self, viz: VizId) -> Option<&PlotPanelState> {
        self.by_viz.get(&viz)
    }

    /// Get or create a plot's state.
    pub fn entry(&mut self, viz: VizId) -> &mut PlotPanelState {
        self.by_viz.entry(viz).or_default()
    }

    /// Return selected variables for a plot.
    pub fn picked(&self, viz: VizId) -> BTreeSet<String> {
        self.by_viz
            .get(&viz)
            .map(|state| state.picked_vars.clone())
            .unwrap_or_default()
    }

    /// Return the scrub time for a plot.
    pub fn scrub(&self, viz: VizId) -> Option<f64> {
        self.by_viz.get(&viz).and_then(|state| state.scrub_time)
    }

    /// Toggle one variable in a plot.
    pub fn toggle_var(&mut self, viz: VizId, var: String) {
        let state = self.entry(viz);
        if !state.picked_vars.insert(var.clone()) {
            state.picked_vars.remove(&var);
        }
    }

    /// Set one variable's selection state.
    pub fn set_var(&mut self, viz: VizId, var: String, on: bool) {
        let state = self.entry(viz);
        if on {
            state.picked_vars.insert(var);
        } else {
            state.picked_vars.remove(&var);
        }
    }

    /// Set a plot's scrub time.
    pub fn set_scrub(&mut self, viz: VizId, time: Option<f64>) {
        self.entry(viz).scrub_time = time;
    }

    /// Return the runs visible in a plot.
    pub fn visible(&self, viz: VizId) -> HashSet<ExperimentId> {
        self.by_viz
            .get(&viz)
            .map(|state| state.visible_experiments.clone())
            .unwrap_or_default()
    }

    /// Test whether a run is visible in a plot.
    pub fn is_visible(&self, viz: VizId, id: ExperimentId) -> bool {
        self.by_viz
            .get(&viz)
            .is_some_and(|state| state.visible_experiments.contains(&id))
    }

    /// Toggle one run's visibility in a plot.
    pub fn toggle_visible(&mut self, viz: VizId, id: ExperimentId) {
        let state = self.entry(viz);
        if !state.visible_experiments.insert(id) {
            state.visible_experiments.remove(&id);
        }
    }

    /// Set one run's visibility in a plot.
    pub fn set_visible(&mut self, viz: VizId, id: ExperimentId, on: bool) {
        let state = self.entry(viz);
        if on {
            state.visible_experiments.insert(id);
        } else {
            state.visible_experiments.remove(&id);
        }
    }

    /// Remove a deleted run from live and archived plot state.
    pub fn forget_experiment(&mut self, id: ExperimentId) {
        for state in self.by_viz.values_mut() {
            state.visible_experiments.remove(&id);
            state.auto_shown.remove(&id);
        }
        for state in self.archived.values_mut() {
            state.visible_experiments.remove(&id);
            state.auto_shown.remove(&id);
        }
    }

    /// Switch a plot to a Twin while preserving each Twin's selections.
    pub fn sync_twin(&mut self, viz: VizId, twin: &TwinId) {
        let needs_swap = match self.by_viz.get(&viz) {
            Some(state) => state.last_twin.as_ref() != Some(twin),
            None => true,
        };
        if !needs_swap {
            return;
        }

        if let Some(previous) = self.by_viz.remove(&viz) {
            if let Some(previous_twin) = previous.last_twin.clone() {
                let worth_keeping = !previous.picked_vars.is_empty()
                    || !previous.visible_experiments.is_empty()
                    || previous.scrub_time.is_some();
                if worth_keeping {
                    self.archived.insert((viz, previous_twin), previous);
                }
            }
        }

        let mut state = self
            .archived
            .remove(&(viz, twin.clone()))
            .unwrap_or_else(|| {
                let mut state = PlotPanelState::default();
                state.visible_experiments.insert(ExperimentId::live());
                state
            });
        state.last_twin = Some(twin.clone());
        self.by_viz.insert(viz, state);
    }
}

/// Most recently rendered plot instance.
#[derive(Resource, Default, Debug, Copy, Clone)]
pub struct ActivePlot(pub Option<VizId>);

impl ActivePlot {
    /// Resolve this optional selection against the host's canonical plot.
    /// The generic state must not embed a Modelica-specific plot id.
    pub fn or_default(self, default: VizId) -> VizId {
        self.0.unwrap_or(default)
    }
}

/// Change-gated cache of trajectory points and the variable catalog.
///
/// The cache is keyed by a Twin and a content signature of its registry. A
/// host supplies the current Twin because document-to-Twin resolution belongs
/// to that host, not to this backend-neutral package.
#[derive(Resource, Default)]
pub struct ExperimentsViewModel {
    built_for: Option<(TwinId, u64, u64)>,
    points: HashMap<(ExperimentId, String), Arc<Vec<[f64; 2]>>>,
    all_vars: BTreeSet<String>,
}

impl ExperimentsViewModel {
    /// Get pre-zipped points for one run and variable.
    pub fn points_for(
        &self,
        experiment: ExperimentId,
        variable: &str,
    ) -> Option<Arc<Vec<[f64; 2]>>> {
        self.points.get(&(experiment, variable.to_owned())).cloned()
    }

    /// Get the cached variable catalog.
    pub fn variables(&self) -> BTreeSet<String> {
        self.all_vars.clone()
    }
}

/// Rebuild the shared trajectory cache when the selected Twin's results
/// change. The caller owns Twin/document resolution and supplies `None` when
/// no document is ready to display.
pub fn populate_experiments_view_model(world: &mut World, twin: Option<&TwinId>) {
    let clear = |world: &mut World| {
        let mut view_model = world.resource_mut::<ExperimentsViewModel>();
        if view_model.built_for.is_some() {
            view_model.built_for = None;
            view_model.points.clear();
            view_model.all_vars.clear();
        }
    };

    let Some(twin) = twin else {
        clear(world);
        return;
    };

    let (signature, run_count) = {
        let Some(registry) = world.get_resource::<ExperimentRegistry>() else {
            clear(world);
            return;
        };
        let runs = registry.list_for_twin(twin);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for experiment in runs {
            experiment.id.hash(&mut hasher);
            match &experiment.status {
                RunStatus::Pending => 0u8.hash(&mut hasher),
                RunStatus::Queued => 1u8.hash(&mut hasher),
                RunStatus::Running { t_current } => {
                    2u8.hash(&mut hasher);
                    t_current.to_bits().hash(&mut hasher);
                }
                RunStatus::Done { wall_time_ms } => {
                    3u8.hash(&mut hasher);
                    wall_time_ms.hash(&mut hasher);
                }
                RunStatus::Failed { error, partial } => {
                    4u8.hash(&mut hasher);
                    error.hash(&mut hasher);
                    partial.hash(&mut hasher);
                }
                RunStatus::Cancelled => 5u8.hash(&mut hasher),
            }
            match &experiment.result {
                Some(result) => {
                    result.times.len().hash(&mut hasher);
                    for (variable, values) in &result.series {
                        variable.hash(&mut hasher);
                        values.len().hash(&mut hasher);
                    }
                }
                None => usize::MAX.hash(&mut hasher),
            }
        }
        (hasher.finish(), runs.len() as u64)
    };

    let key = (twin.clone(), signature, run_count);
    if world.resource::<ExperimentsViewModel>().built_for.as_ref() == Some(&key) {
        return;
    }

    let (points, all_vars) = {
        let registry = world.resource::<ExperimentRegistry>();
        let mut points = HashMap::new();
        let mut all_vars = BTreeSet::new();
        for experiment in registry.list_for_twin(twin) {
            let Some(result) = &experiment.result else {
                continue;
            };
            for (variable, values) in &result.series {
                all_vars.insert(variable.clone());
                let zipped = Arc::new(
                    result
                        .times
                        .iter()
                        .zip(values.iter())
                        .map(|(time, value)| [*time, *value])
                        .collect(),
                );
                points.insert((experiment.id, variable.clone()), zipped);
            }
        }
        (points, all_vars)
    };

    let mut view_model = world.resource_mut::<ExperimentsViewModel>();
    view_model.points = points;
    view_model.all_vars = all_vars;
    view_model.built_for = Some(key);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plot_state_restores_per_twin_selection() {
        let mut states = PlotPanelStates::default();
        let viz = VizId(7);
        let first = TwinId("first".into());
        let second = TwinId("second".into());

        states.sync_twin(viz, &first);
        states.set_var(viz, "position.x".into(), true);
        states.sync_twin(viz, &second);
        assert!(states.picked(viz).is_empty());
        states.sync_twin(viz, &first);

        assert!(states.picked(viz).contains("position.x"));
    }
}
