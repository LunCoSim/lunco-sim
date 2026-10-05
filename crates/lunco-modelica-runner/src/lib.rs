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
#[cfg(test)]
use lunco_experiments::{ExperimentOrigin, ExperimentOrigins};
#[cfg(test)]
use lunco_workspace::PinnedDocumentRuntimeOwner;
use std::sync::Arc;

pub mod run_bounds;
pub mod runner;

pub use run_bounds::{bounds_from_annotation, resolve_setup_bounds, resolve_setup_bounds_in};
#[cfg(target_arch = "wasm32")]
pub use runner::pump_wasm_forwarders;
pub use runner::{
    DEFAULT_TOLERANCE, DetectedInput, DetectedParam, ExperimentDraft, ExperimentDrafts,
    ExperimentSettings, ModelSource, ModelicaRunner, PendingHandles, PendingRun, PlaybackEntities,
    RunConfigurationError, RunSink, apply_experiment_settings, apply_value_bindings_to_dae,
    detect_top_level_inputs, detect_top_level_literal_parameters, drain_pending_handles, drive_run,
    stepper_options_from_bounds,
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
        app.init_resource::<PendingHandles>();
        app.init_resource::<PlaybackEntities>();
        app.add_observer(cancel_closed_twin_runs);
        app.add_observer(cancel_retired_replication_runs);
        app.add_systems(
            Last,
            cancel_removed_experiment_handles
                .after(lunco_experiments::ExperimentRegistryMaintenanceSet)
                .run_if(on_message::<lunco_experiments::ExperimentRemoved>),
        );
        use lunco_settings::AppSettingsExt;
        app.register_settings_section::<ExperimentSettings>();
        app.add_systems(Update, (apply_experiment_settings, drain_pending_handles));
    }
}

/// A closed runtime owner cancels unfinished work without deleting history.
fn cancel_closed_twin_runs(
    trigger: On<lunco_workspace::TwinClosed>,
    mut pending: ResMut<PendingHandles>,
    mut experiments: ResMut<lunco_experiments::ExperimentRegistry>,
    mut cancelled: MessageWriter<lunco_experiments::RunCancelled>,
    runner: Option<Res<ModelicaRunnerResource>>,
) {
    cancel_runtime_runs(
        &lunco_workspace::DocumentRuntimeOwner::LocalTwin(trigger.event().twin),
        &mut pending,
        &mut experiments,
        &mut cancelled,
        runner.as_deref(),
    );
}

fn cancel_retired_replication_runs(
    trigger: On<lunco_core_session::ReplicationOwnerRetired>,
    mut pending: ResMut<PendingHandles>,
    mut experiments: ResMut<lunco_experiments::ExperimentRegistry>,
    mut cancelled: MessageWriter<lunco_experiments::RunCancelled>,
    runner: Option<Res<ModelicaRunnerResource>>,
) {
    cancel_runtime_runs(
        &lunco_workspace::DocumentRuntimeOwner::Replicated(trigger.event().owner.clone()),
        &mut pending,
        &mut experiments,
        &mut cancelled,
        runner.as_deref(),
    );
}

fn cancel_runtime_runs(
    owner: &lunco_workspace::DocumentRuntimeOwner,
    pending: &mut PendingHandles,
    experiments: &mut lunco_experiments::ExperimentRegistry,
    cancelled: &mut MessageWriter<lunco_experiments::RunCancelled>,
    runner: Option<&ModelicaRunnerResource>,
) {
    #[cfg(target_arch = "wasm32")]
    let _ = runner;
    pending.0.retain(|handle| {
        if !handle.origin.belongs_to_runtime(owner) {
            return true;
        }
        handle.cancel();
        if experiments
            .get(handle.run_id)
            .is_some_and(|run| !run.status.is_terminal())
        {
            experiments.set_status(handle.run_id, lunco_experiments::RunStatus::Cancelled);
            cancelled.write(lunco_experiments::RunCancelled {
                experiment_id: handle.run_id,
                origin: handle.origin.clone(),
            });
        }
        false
    });
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(runner) = runner {
        if let Err(error) = runner.0.retire_runtime_cache(owner) {
            bevy::log::error!("{error}");
        }
    }
}

fn cancel_removed_experiment_handles(
    mut removed: MessageReader<lunco_experiments::ExperimentRemoved>,
    mut pending: ResMut<PendingHandles>,
) {
    for event in removed.read() {
        pending.0.retain(|handle| {
            if handle.run_id == event.experiment_id {
                handle.cancel();
                false
            } else {
                true
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_experiments::{
        ExperimentRegistry, ModelRef, RunBounds, RunHandle, RunStatus, RunUpdate,
    };
    use std::sync::atomic::{AtomicBool, Ordering};

    fn admitted_run(
        registry: &mut ExperimentRegistry,
        origins: &mut ExperimentOrigins,
        twin: Option<lunco_workspace::TwinId>,
        doc: u64,
    ) -> lunco_experiments::ExperimentId {
        origins.insert_new(
            registry,
            ExperimentOrigin::LocalDocument(PinnedDocumentRuntimeOwner {
                document: lunco_doc::DocumentId::new(doc),
                runtime: twin.map_or(
                    lunco_workspace::DocumentRuntimeOwner::Application,
                    lunco_workspace::DocumentRuntimeOwner::LocalTwin,
                ),
            }),
            lunco_experiments::TwinId("history".into()),
            ModelRef("Plant".into()),
            Default::default(),
            Default::default(),
            RunBounds::default(),
        )
    }

    #[test]
    fn closing_twin_cancels_exact_owner_and_retires_late_updates() {
        let a = lunco_workspace::TwinId::new(1);
        let b = lunco_workspace::TwinId::new(2);
        let mut registry = ExperimentRegistry::new();
        let mut origins = ExperimentOrigins::default();
        let owned = admitted_run(&mut registry, &mut origins, Some(a), 1);
        let other = admitted_run(&mut registry, &mut origins, Some(b), 2);
        let application = admitted_run(&mut registry, &mut origins, None, 3);
        let completed = admitted_run(&mut registry, &mut origins, Some(a), 4);
        for id in [owned, other, application] {
            registry.set_status(id, RunStatus::Queued);
        }
        registry.set_status(completed, RunStatus::Done { wall_time_ms: 1 });
        let mut pending = PendingHandles::default();
        let mut flags = Vec::new();
        let mut senders = Vec::new();
        for id in [owned, other, application] {
            let (tx, rx) = crossbeam_channel::unbounded();
            let flag = Arc::new(AtomicBool::new(false));
            let cancel = flag.clone();
            pending.0.push(PendingRun {
                origin: origins.get(&id).expect("admitted origin").clone(),
                handle: RunHandle {
                    run_id: id,
                    progress_rx: rx,
                    cancel: Box::new(move || {
                        cancel.store(true, Ordering::SeqCst);
                    }),
                },
            });
            senders.push(tx);
            flags.push(flag);
        }
        let mut app = App::new();
        app.add_plugins(lunco_experiments::ExperimentsPlugin)
            .insert_resource(registry)
            .insert_resource(origins)
            .insert_resource(pending)
            .add_observer(cancel_closed_twin_runs)
            .add_systems(Update, drain_pending_handles);
        app.world_mut().trigger(lunco_workspace::TwinClosed {
            twin: a,
            root: Default::default(),
            was_active: true,
        });
        assert!(flags[0].load(Ordering::SeqCst));
        assert!(!flags[1].load(Ordering::SeqCst));
        assert!(!flags[2].load(Ordering::SeqCst));
        assert!(
            senders[0]
                .send(RunUpdate::Completed(lunco_experiments::RunResult {
                    times: vec![0.0],
                    series: Default::default(),
                    meta: Default::default()
                }))
                .is_err()
        );
        app.update();
        let registry = app.world().resource::<ExperimentRegistry>();
        assert!(matches!(
            registry.get(owned).expect("owned").status,
            RunStatus::Cancelled
        ));
        assert!(matches!(
            registry.get(other).expect("other").status,
            RunStatus::Queued
        ));
        assert!(matches!(
            registry.get(application).expect("application").status,
            RunStatus::Queued
        ));
        assert!(matches!(
            registry.get(completed).expect("completed").status,
            RunStatus::Done { .. }
        ));
        assert_eq!(
            app.world().resource::<ExperimentOrigins>().iter().count(),
            4
        );
    }

    #[test]
    fn owner_terminal_status_retires_buffered_worker_updates() {
        let mut registry = ExperimentRegistry::new();
        let mut origins = ExperimentOrigins::default();
        let id = admitted_run(&mut registry, &mut origins, None, 1);
        registry.set_status(id, RunStatus::Cancelled);
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(RunUpdate::Progress {
            t_current: 2.0,
            delta: None,
        })
        .expect("buffered progress");
        let flag = Arc::new(AtomicBool::new(false));
        let cancel = flag.clone();
        let pending = PendingRun {
            origin: origins.get(&id).expect("origin").clone(),
            handle: RunHandle {
                run_id: id,
                progress_rx: rx,
                cancel: Box::new(move || {
                    cancel.store(true, Ordering::SeqCst);
                }),
            },
        };
        let mut app = App::new();
        app.add_plugins(lunco_experiments::ExperimentsPlugin)
            .insert_resource(registry)
            .insert_resource(origins)
            .insert_resource(PendingHandles(vec![pending]))
            .add_systems(Update, drain_pending_handles);
        app.update();
        assert!(flag.load(Ordering::SeqCst));
        assert!(app.world().resource::<PendingHandles>().0.is_empty());
        assert!(matches!(
            app.world()
                .resource::<ExperimentRegistry>()
                .get(id)
                .expect("history")
                .status,
            RunStatus::Cancelled
        ));
        assert!(tx.send(RunUpdate::Cancelled).is_err());
    }
}
