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

        #[cfg(not(target_arch = "wasm32"))]
        thread::spawn(move || lunco_modelica_worker::worker::modelica_worker(rx_cmd, tx_res));

        #[cfg(not(target_arch = "wasm32"))]
        app.insert_resource(ModelicaChannels {
            tx: tx_cmd,
            rx: rx_res,
        });

        #[cfg(target_arch = "wasm32")]
        {
            if let Err(error) = lunco_modelica_runner::install_worker_run_transport(
                lunco_modelica_runner::WorkerRunTransport {
                    register_run_sender: worker_transport::register_run_sender,
                    dispatch_run_fast: worker_transport::dispatch_run_fast,
                    dispatch_cancel_run: worker_transport::dispatch_cancel_run,
                },
            ) {
                bevy::log::warn!("{error}");
            }
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

        app.init_resource::<lunco_signal::SimRegistry>();
        app.init_resource::<SimSampleStream>();
        app.add_message::<ModelicaNotice>();
        app.add_message::<CompileRequested>();

        app.add_plugins(lunco_modelica_runner::ModelicaRunnerPlugin);

        app.configure_sets(Update, ModelicaSet::HandleResponses);
        app.configure_sets(FixedUpdate, ModelicaSet::SpawnRequests);
        app.add_plugins(lunco_modelica_telemetry::ModelicaTelemetryPlugin);
        app.init_resource::<lunco_modelica_worker::worker::CosimLag>();
        app.register_type::<ModelicaModel>()
            .add_observer(lunco_modelica_worker::worker::on_remove_modelica)
            .add_systems(
                Update,
                lunco_modelica_worker::worker::handle_modelica_responses
                    .in_set(ModelicaSet::HandleResponses),
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
