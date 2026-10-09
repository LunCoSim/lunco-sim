//! Modelica execution and transport adapters.
//!
//! This package owns the expensive, change-prone part of Modelica integration:
//! solver host assembly and the browser worker wire.
//! [`lunco_modelica_core`] remains the reusable compiler/document package;
//! [`lunco_modelica_worker`] owns the stateful worker engine.
//! The shared parser/source-library callback seam is owned by
//! [`lunco_modelica_library`].

use bevy::prelude::*;
use crossbeam_channel::unbounded;
#[cfg(target_arch = "wasm32")]
use lunco_modelica_library::worker_bridge::ModelicaWorkerBridge;
use lunco_modelica_runtime::{
    CompileRequested, ModelicaChannels, ModelicaModel, ModelicaNotice, ModelicaSet, SimSampleStream,
};
#[cfg(not(target_arch = "wasm32"))]
use std::thread;

#[cfg(target_arch = "wasm32")]
pub mod worker_transport;

/// Make the built-in solver descriptors available to query and UI surfaces.
///
/// Registration is owned by the execution package because these descriptors
/// describe executable backends, not compiler/document state.
pub fn ensure_builtin_solvers() {
    lunco_modelica_solver::solver_backends::ensure_builtin_solvers();
}

/// Plugin that installs the Modelica solver worker and Fast Run runtime.
///
/// [`lunco_modelica_core::ModelicaCorePlugin`] must be composed separately.
/// Keeping the dependency explicit lets compiler-only consumers avoid the
/// solver and worker dependency closure entirely.
pub struct ModelicaExecutionPlugin;

impl Plugin for ModelicaExecutionPlugin {
    fn build(&self, app: &mut App) {
        let (tx_cmd, rx_cmd) = unbounded();
        let (tx_res, rx_res) = unbounded();
        app.init_resource::<lunco_modelica_runtime::ModelicaCacheLimits>();
        let cache_limits = *app
            .world()
            .resource::<lunco_modelica_runtime::ModelicaCacheLimits>();

        #[cfg(not(target_arch = "wasm32"))]
        {
            app.init_resource::<lunco_modelica_worker::worker::PreparedSolveDiskLimits>();
            let disk_limits = *app
                .world()
                .resource::<lunco_modelica_worker::worker::PreparedSolveDiskLimits>();
            let failure_sender = tx_res.clone();
            let spawn = thread::Builder::new()
                .name("modelica-worker".into())
                .spawn(move || {
                    let failure_sender = tx_res.clone();
                    if let Err(payload) =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            lunco_modelica_worker::worker::modelica_worker(
                                rx_cmd,
                                tx_res,
                                disk_limits,
                                cache_limits,
                            )
                        }))
                    {
                        let detail = payload
                            .downcast_ref::<String>()
                            .map(String::as_str)
                            .or_else(|| payload.downcast_ref::<&str>().copied())
                            .unwrap_or("non-string panic payload");
                        let _ = failure_sender.send(
                            lunco_modelica_runtime::ModelicaResult::worker_failure(format!(
                                "Modelica worker panicked: {detail}"
                            )),
                        );
                    }
                });
            if let Err(error) = spawn {
                let _ =
                    failure_sender.send(lunco_modelica_runtime::ModelicaResult::worker_failure(
                        format!("cannot start Modelica worker: {error}"),
                    ));
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        app.insert_resource(ModelicaChannels {
            tx: tx_cmd,
            rx: rx_res,
        });

        #[cfg(target_arch = "wasm32")]
        {
            if let Err(error) = worker_transport::configure_cache_limits(cache_limits) {
                worker_transport::fail_worker_pipeline(error.clone());
                let _ = tx_res.send(lunco_modelica_runtime::ModelicaResult::worker_failure(
                    error,
                ));
            }
            if let Err(error) = lunco_modelica_runner::install_worker_run_transport(
                lunco_modelica_runner::WorkerRunTransport {
                    register_run_sender: worker_transport::register_run_sender,
                    dispatch_run_fast: worker_transport::dispatch_run_fast,
                    dispatch_cancel_run: worker_transport::dispatch_cancel_run,
                },
            ) {
                bevy::log::warn!("{error}");
            }
            app.insert_resource(lunco_experiments::artifact::ArtifactWorkerTransport {
                dispatch: worker_transport::dispatch_artifact,
                retire: worker_transport::retire_artifact_leases,
                discard: worker_transport::discard_artifact_lease,
                clear: worker_transport::clear_artifact_leases,
            });
            let _ = worker_transport::register_result_sender(tx_res.clone());
            let _ = worker_transport::register_command_sender(tx_cmd.clone());
            app.insert_resource(ModelicaChannels {
                tx: tx_cmd,
                rx: rx_res,
                rx_cmd,
                tx_res,
            });
            app.insert_resource(ModelicaWorkerBridge::new(
                worker_transport::dispatch_parse_to_worker,
                worker_transport::try_recv_parse_done,
                worker_transport::try_recv_parse_failed,
                worker_transport::prewarm_pool_on_source_bundle_ready,
                worker_transport::install_library_compressed_in_worker,
                worker_transport::load_library_index_in_worker,
                worker_transport::reset_worker_pipeline,
                worker_transport::fail_worker_pipeline,
            ));
        }

        app.init_resource::<lunco_modelica_runtime::ModelicaWorkerFailure>();
        app.init_resource::<lunco_signal::SimRegistry>();
        app.init_resource::<SimSampleStream>();
        app.init_resource::<lunco_modelica_runtime::ModelicaStepDiagnostics>();
        app.add_message::<ModelicaNotice>();
        app.add_message::<CompileRequested>();

        app.add_plugins(lunco_modelica_runner::ModelicaRunnerPlugin);

        app.configure_sets(
            First,
            ModelicaSet::HandleResponses
                .after(bevy::ecs::message::MessageUpdateSystems)
                .before(lunco_time::ClockProjectionSet)
                .in_set(lunco_core::RuntimeCycleSet::Lifecycle),
        );
        app.configure_sets(
            Update,
            ModelicaSet::AdmitCompileRequests.in_set(lunco_core::RuntimeCycleSet::Lifecycle),
        );
        app.init_resource::<lunco_core_runtime::SimulationProgress>()
            .add_systems(
                PreUpdate,
                lunco_modelica_worker::worker::reconcile_modelica_preparation_progress
                    .in_set(lunco_core::RuntimeCycleSet::Lifecycle),
            );
        app.configure_sets(FixedUpdate, ModelicaSet::SpawnRequests);
        app.add_plugins(lunco_modelica_telemetry::ModelicaTelemetryPlugin);
        app.init_resource::<lunco_modelica_worker::worker::CosimLag>();
        app.register_type::<ModelicaModel>()
            .add_observer(lunco_modelica_worker::worker::on_remove_modelica)
            .add_observer(lunco_modelica_worker::worker::retire_closed_twin_models)
            .add_observer(lunco_modelica_worker::worker::retire_replication_models)
            .add_systems(
                First,
                lunco_modelica_worker::worker::handle_modelica_responses
                    .in_set(ModelicaSet::HandleResponses),
            )
            .add_systems(
                Update,
                (
                    lunco_modelica_worker::worker::request_modelica_compiles,
                    lunco_modelica_worker::worker::dispatch_modelica_compile_requests,
                )
                    .chain()
                    .in_set(ModelicaSet::AdmitCompileRequests),
            )
            .add_systems(
                FixedUpdate,
                lunco_modelica_worker::worker::spawn_modelica_requests
                    .in_set(ModelicaSet::SpawnRequests)
                    .run_if(lunco_time::simulation_is_running),
            );

        #[cfg(target_arch = "wasm32")]
        {
            app.add_systems(Update, worker_transport::pump_commands_to_worker);
            app.add_systems(Update, |_world: &mut World| {
                worker_transport::pump_worker_respawns();
            });
            app.add_systems(Update, |_world: &mut World| {
                lunco_modelica_runner::pump_wasm_forwarders();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct ClockProjectionObservations(Vec<bool>);

    fn observe_clock_projection(
        barrier: Res<lunco_core_runtime::SimulationBarrier>,
        mut observations: ResMut<ClockProjectionObservations>,
    ) {
        observations.0.push(barrier.held);
    }

    #[test]
    fn worker_completion_precedes_clock_projection_and_rejects_stale_sessions() {
        let mut app = App::new();
        app.add_plugins(ModelicaExecutionPlugin)
            .init_resource::<lunco_core_runtime::SimulationBarrier>()
            .init_resource::<ClockProjectionObservations>()
            .add_systems(
                First,
                observe_clock_projection.in_set(lunco_time::ClockProjectionSet),
            );
        let (results, rx) = unbounded();
        let (tx, _commands) = unbounded();
        app.insert_resource(ModelicaChannels { tx, rx });
        let entity = app
            .world_mut()
            .spawn(ModelicaModel {
                session_id: 1,
                paused: false,
                is_compiled: true,
                is_stepping: true,
                in_flight_step: Some(lunco_modelica_runtime::InFlightModelicaStep {
                    step_id: 1,
                    start_time: 0.0,
                    stop_time: 0.05,
                    sampled_inputs: Vec::new(),
                    submitted_at: web_time::Instant::now(),
                }),
                ..Default::default()
            })
            .id();
        app.world_mut()
            .resource_mut::<lunco_core_runtime::SimulationBarrier>()
            .held = true;

        app.world_mut().run_schedule(First);
        results
            .send(lunco_modelica_runtime::ModelicaResult {
                entity,
                session_id: 0,
                step_id: Some(1),
                new_time: 0.05,
                ..Default::default()
            })
            .unwrap();
        app.world_mut().run_schedule(First);
        results
            .send(lunco_modelica_runtime::ModelicaResult {
                entity,
                session_id: 1,
                step_id: Some(1),
                new_time: 0.05,
                ..Default::default()
            })
            .unwrap();
        app.world_mut().run_schedule(First);

        assert_eq!(
            app.world().resource::<ClockProjectionObservations>().0,
            [true, true, false],
            "clock projection sees the validated completion in the same update"
        );
        let model = app.world().get::<ModelicaModel>(entity).unwrap();
        assert_eq!(model.current_time, 0.05);
        assert!(!model.is_stepping);
        assert_eq!(model.last_accepted_step.as_ref().unwrap().step_id, 1);
    }

    #[test]
    fn execution_plugin_is_distinct_from_compiler_plugin() {
        let mut app = App::new();
        app.add_plugins(ModelicaExecutionPlugin);
        assert!(app.world().contains_resource::<ModelicaChannels>());
        assert!(
            app.world()
                .contains_resource::<lunco_modelica_runner::ModelicaRunnerResource>()
        );
    }
}
