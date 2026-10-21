//! Native live-session mailboxes. Solver state stays on its constructing thread.

use super::thread_pool::Lane;
use super::{LiveBuildPlan, LiveStepper, WorkerPreparationResult, build_prepared_stepper};
use crossbeam_channel::{Receiver, Sender, bounded};
use rumoca_sim::SimulationDiagnosticError;
use std::collections::HashMap;
use std::sync::Arc;

type Sessions = HashMap<u64, LiveStepper>;

pub(super) struct LiveConstructionResult {
    pub id: u64,
    pub result: Result<super::BuiltStepper<SessionHandle>, SimulationDiagnosticError>,
}

struct CompletedStep {
    time: f64,
    outputs: Result<Vec<(String, f64)>, SimulationDiagnosticError>,
    elapsed_ns: u64,
}

struct PendingStep {
    id: u64,
    dt_bits: u64,
    stop_bits: u64,
    receipt: Result<Receiver<Result<CompletedStep, SimulationDiagnosticError>>, String>,
}

/// An admitted session identity and last observed clock; it owns no solver state.
pub(super) struct SessionHandle {
    lane: Lane<Sessions>,
    id: u64,
    time: f64,
    input_names: Vec<String>,
    pending: Option<PendingStep>,
    last_step_duration_ns: Option<u64>,
}

fn owner_error(message: impl Into<String>) -> SimulationDiagnosticError {
    SimulationDiagnosticError::Solver(message.into())
}

pub(super) fn construct(
    id: u64,
    lane: Lane<Sessions>,
    component: Arc<rumoca_ir_solve::fmi::FmiComponent>,
    plan: LiveBuildPlan,
    native_cache: super::native_code_cache::SharedNativeCodeCache,
    tx: Sender<WorkerPreparationResult>,
) -> Result<(), String> {
    let session_lane = lane.clone();
    let queued_at = web_time::Instant::now();
    lane.submit(move |sessions| {
        let queue_wait_ns = queued_at.elapsed().as_nanos() as u64;
        let _span = bevy::log::info_span!(
            "modelica_live_construction_job",
            construction_id = id,
            queue_wait_us = queue_wait_ns / 1000
        )
        .entered();
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            native_cache.construct(|| build_prepared_stepper(&component, plan))
        }))
        .unwrap_or_else(|payload| {
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown panic payload");
            Err(owner_error(format!(
                "live session construction panicked: {message}"
            )))
        });
        let result = built.map(
            |((stepper, snapshot, metadata, symbols, mut profile), native_profile)| {
                profile.queue_wait_ns = Some(queue_wait_ns);
                profile.native_code_cache = Some(native_profile);
                let handle = SessionHandle {
                    lane: session_lane,
                    id,
                    time: stepper.time(),
                    input_names: stepper.input_names().to_vec(),
                    pending: None,
                    last_step_duration_ns: None,
                };
                sessions.insert(id, stepper);
                (handle, snapshot, metadata, symbols, profile)
            },
        );
        let _ = tx.send(WorkerPreparationResult::Live(LiveConstructionResult {
            id,
            result,
        }));
    })
}

impl SessionHandle {
    pub(super) fn time(&self) -> f64 {
        self.time
    }
    pub(super) fn input_names(&self) -> &[String] {
        &self.input_names
    }
    pub(super) fn last_step_duration_ns(&self) -> Option<u64> {
        self.last_step_duration_ns
    }
    pub(super) fn has_pending_step(&self) -> bool {
        self.pending.is_some()
    }

    /// Transfer one complete transaction to its owner before ordered admission.
    pub(super) fn start_step(
        &mut self,
        step_id: u64,
        start_time: f64,
        stop_time: f64,
        dt: f64,
        inputs: Vec<(String, f64)>,
    ) -> Result<(), SimulationDiagnosticError> {
        if self.pending.is_some() {
            return Err(owner_error(
                "a live transaction is already pending on this owner",
            ));
        }
        self.last_step_duration_ns = None;
        let (tx, rx) = bounded(1);
        let id = self.id;
        let submitted = self.lane.submit(move |sessions| {
            // Tracy includes span fields in source-location identity. Keep the
            // per-step zone bounded; transaction ids live in the receipts.
            let _span = bevy::log::info_span!("modelica_live_step_job").entered();
            let Some(stepper) = sessions.get_mut(&id) else {
                let _ = tx.send(Err(owner_error(format!(
                    "live session {id} is absent from its owner"
                ))));
                return;
            };
            let started = web_time::Instant::now();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                super::validate_step_request(stepper.time(), start_time, stop_time, dt)
                    .map_err(owner_error)?;
                super::advance_checked_step(stepper, &inputs, stop_time)
            }));
            let outputs = match outcome {
                Ok(outputs) => outputs,
                Err(payload) => {
                    let message = payload
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown panic payload");
                    Err(owner_error(format!(
                        "live session transaction panicked: {message}"
                    )))
                }
            };
            let receipt = CompletedStep {
                time: stepper.time(),
                outputs,
                elapsed_ns: started.elapsed().as_nanos() as u64,
            };
            if receipt.outputs.is_err() {
                sessions.remove(&id);
            }
            let _ = tx.send(Ok(receipt));
        });
        self.pending = Some(PendingStep {
            id: step_id,
            dt_bits: dt.to_bits(),
            stop_bits: stop_time.to_bits(),
            receipt: submitted.map(|()| rx),
        });
        Ok(())
    }

    pub(super) fn advance_checked(
        &mut self,
        step_id: u64,
        inputs: Vec<(String, f64)>,
        dt: f64,
        stop_time: f64,
    ) -> Result<Vec<(String, f64)>, SimulationDiagnosticError> {
        if self.pending.is_none() {
            self.start_step(step_id, self.time, stop_time, dt, inputs)?;
        } else if !inputs.is_empty() {
            return Err(owner_error(
                "the admitted live transaction already owns its inputs",
            ));
        }
        let Some(pending) = self.pending.take() else {
            return Err(owner_error("live transaction has no admitted owner work"));
        };
        if pending.id != step_id
            || pending.dt_bits != dt.to_bits()
            || pending.stop_bits != stop_time.to_bits()
        {
            return Err(owner_error(
                "live transaction receipt does not match its ordered command",
            ));
        }
        let receipt =
            pending.receipt.map_err(owner_error)?.recv().map_err(|_| {
                owner_error("live session owner disconnected during a transaction")
            })??;
        self.time = receipt.time;
        self.last_step_duration_ns = Some(receipt.elapsed_ns);
        receipt.outputs
    }

    pub(super) fn capture_initial_inputs(
        &self,
    ) -> Result<Vec<(String, f64)>, SimulationDiagnosticError> {
        let (tx, rx) = bounded(1);
        let id = self.id;
        self.lane
            .submit(move |sessions| {
                let result = sessions
                    .get(&id)
                    .ok_or_else(|| {
                        owner_error(format!("live session {id} is absent from its owner"))
                    })
                    .and_then(super::capture_initial_inputs);
                let _ = tx.send(result);
            })
            .map_err(owner_error)?;
        rx.recv()
            .map_err(|_| owner_error("live session owner disconnected during input observation"))?
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        let id = self.id;
        let _ = self.lane.submit(move |sessions| {
            sessions.remove(&id);
        });
    }
}
