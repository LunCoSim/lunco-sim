//! Modelica experiment scheduling and shared run execution.
//!
//! This package owns the backend-specific implementation of the generic
//! [`lunco_experiments::ExperimentRunner`] contract: source snapshots,
//! compile-once DAE caching, native scheduling, shared numerical run paths,
//! and the small Bevy resources consumed by API/UI hosts. Worker lifecycle and
//! wire transport stay in [`lunco_modelica_execution`]; on wasm that host
//! installs the typed transport callbacks exposed here.

use bevy::prelude::*;
#[cfg(target_arch = "wasm32")]
use crossbeam_channel::Sender;
#[cfg(target_arch = "wasm32")]
use lunco_experiments::{ExperimentId, RunBounds, RunUpdate};
use std::sync::Arc;

pub mod run_bounds;
pub mod runner;

pub use run_bounds::{bounds_from_annotation, resolve_setup_bounds, resolve_setup_bounds_in};
#[cfg(target_arch = "wasm32")]
pub use runner::pump_wasm_forwarders;
pub use runner::{
    DEFAULT_TOLERANCE, DetectedInput, DetectedParam, ExperimentDraft, ExperimentDrafts,
    ExperimentSettings, ExperimentSources, ModelDefaults, ModelSource, ModelicaRunner,
    PendingHandles, PlaybackEntities, RunSink, apply_experiment_settings,
    apply_value_bindings_to_dae, detect_top_level_inputs, detect_top_level_literal_parameters,
    drain_pending_handles, drive_run, stepper_options_from_bounds,
};

/// Bevy resource wrapping the singleton [`ModelicaRunner`].
///
/// The `Arc` lets UI/API callers clone the handle without holding a mutable
/// world borrow while a run is queued or executing.
#[derive(Resource, Clone)]
pub struct ModelicaRunnerResource(pub Arc<ModelicaRunner>);

/// Typed callbacks supplied by the wasm worker host.
///
/// The runner owns scheduling and run handles; the execution host owns the
/// browser worker pool. Keeping this contract as function pointers avoids a
/// dependency cycle between those packages and makes the transport boundary
/// explicit.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy)]
pub struct WorkerRunTransport {
    /// Register the per-run update channel before dispatch.
    pub register_run_sender: fn(ExperimentId, Sender<RunUpdate>),
    /// Post a Fast Run request to the worker pool.
    pub dispatch_run_fast: fn(
        ExperimentId,
        String,
        String,
        String,
        Vec<(String, String)>,
        std::collections::BTreeMap<lunco_experiments::ParamPath, lunco_experiments::ParamValue>,
        std::collections::BTreeMap<lunco_experiments::ParamPath, lunco_experiments::ParamValue>,
        RunBounds,
    ) -> bool,
    /// Route cancellation to the worker owning the run.
    pub dispatch_cancel_run: fn(ExperimentId),
}

#[cfg(target_arch = "wasm32")]
static WORKER_RUN_TRANSPORT: std::sync::OnceLock<WorkerRunTransport> = std::sync::OnceLock::new();

/// Install the worker callbacks used by wasm Fast Runs.
///
/// Installation is explicit and one-shot: the execution host must configure
/// it before adding [`ModelicaRunnerPlugin`]. A missing installation is
/// reported as a failed run rather than leaving the scheduler slot occupied.
#[cfg(target_arch = "wasm32")]
pub fn install_worker_run_transport(transport: WorkerRunTransport) -> Result<(), &'static str> {
    WORKER_RUN_TRANSPORT
        .set(transport)
        .map_err(|_| "Modelica worker run transport was already installed")
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn worker_run_transport() -> Option<WorkerRunTransport> {
    WORKER_RUN_TRANSPORT.get().copied()
}

/// Plugin installing the experiment registry adapter and runner state.
///
/// Worker execution hosts compose this plugin with their own worker/plugin
/// transport. Compiler-only hosts do not need either package.
pub struct ModelicaRunnerPlugin;

impl Plugin for ModelicaRunnerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(lunco_experiments::ExperimentsPlugin);
        app.insert_resource(ModelicaRunnerResource(Arc::new(ModelicaRunner::new())));
        app.init_resource::<ExperimentDrafts>();
        app.init_resource::<ExperimentSources>();
        app.init_resource::<PendingHandles>();
        app.init_resource::<PlaybackEntities>();
        use lunco_settings::AppSettingsExt;
        app.register_settings_section::<ExperimentSettings>();
        app.add_systems(Update, (apply_experiment_settings, drain_pending_handles));
    }
}
