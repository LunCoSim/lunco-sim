//! Deterministic fixed-step Modelica integration for client prediction.
//!
//! Rumoca's public `SimulationSession` is an adaptive session. Supplying it a
//! nominal `dt` does not turn it into a fixed-step solver: it still chooses
//! internal steps from error estimates and event boundaries. Prediction needs a
//! stronger contract, so this module owns the integration loop explicitly.
//!
//! The backend is intentionally qualified. It accepts continuous, event-free,
//! external-table-free models and evaluates them with classical RK4 at exactly
//! four derivative stages per fixed step. Event/discrete models are rejected at
//! construction because silently sampling them would be a semantic error, not a
//! solver improvement.

use indexmap::IndexMap;
use rumoca_compile::compile::Dae;
use rumoca_eval_solve::projection_policy::{
    ALGEBRAIC_REFRESH_MAX_ITERS, ALGEBRAIC_REFRESH_TOLERANCE,
};
use rumoca_ir_solve::SolveModel;
use rumoca_sim::{SessionState, SimOptions, SimulationDiagnosticError};
use rumoca_solver::SolveRuntime;

/// A fixed-step RK4 session over a continuous, event-free Rumoca solve model.
pub struct FixedStepSession {
    runtime: SolveRuntime,
    state: Vec<f64>,
    params: Vec<f64>,
    time_origin: f64,
    step_index: u64,
    fixed_dt: f64,
}

impl FixedStepSession {
    /// Lower and prepare a DAE for the fixed-step backend.
    pub fn new(dae: &Dae, options: SimOptions) -> Result<Self, SimulationDiagnosticError> {
        let lower_started = web_time::Instant::now();
        let component = rumoca_sim::prepare_fmi_component(dae, &options)?;
        let lower_elapsed = lower_started.elapsed();
        Self::from_continuous_model(
            component.runtime_view().model(),
            options,
            Some(lower_elapsed),
        )
    }

    /// Prepare a qualified fixed-step session from a checked FMI runtime view.
    /// The worker retains the checked component across resets and equivalent
    /// instances. `SolveRuntime` copies its executable representation into this
    /// session; the owner then validates the continuous fixed-step subset.
    pub(crate) fn from_continuous_model(
        model: &SolveModel,
        options: SimOptions,
        lower_elapsed: Option<web_time::Duration>,
    ) -> Result<Self, SimulationDiagnosticError> {
        let fixed_dt = options.dt.ok_or_else(|| {
            SimulationDiagnosticError::Solver(
                "fixed-rk4 requires an explicit positive SimOptions::dt".into(),
            )
        })?;
        if !fixed_dt.is_finite() || fixed_dt <= 0.0 {
            return Err(SimulationDiagnosticError::Solver(
                "fixed-rk4 requires a finite positive SimOptions::dt".into(),
            ));
        }
        if !options.t_start.is_finite() {
            return Err(SimulationDiagnosticError::Solver(
                "fixed-rk4 requires a finite start time".into(),
            ));
        }

        reject_unsupported_constructs(model)?;
        let runtime_started = web_time::Instant::now();
        let runtime = SolveRuntime::new(model)?;
        let runtime_elapsed = runtime_started.elapsed();
        let state_count = model.state_scalar_count();
        if model.initial_y.len() < state_count {
            return Err(SimulationDiagnosticError::Solver(format!(
                "fixed-rk4 initial solver vector has {} values, but the model requires {} state values",
                model.initial_y.len(),
                state_count,
            )));
        }
        log::info!(
            "[fixed-rk4] prepared solve runtime: lower={} runtime={runtime_elapsed:?} \
             states={} solver_slots={} algebraic_slots={}",
            lower_elapsed
                .map(|elapsed| format!("{elapsed:?}"))
                .unwrap_or_else(|| "cached".to_string()),
            state_count,
            model.solver_scalar_count(),
            model.solver_scalar_count().saturating_sub(state_count),
        );
        let initial_state = model.initial_y[..state_count].to_vec();

        Ok(Self {
            runtime,
            state: initial_state,
            params: model.parameters.clone(),
            time_origin: options.t_start,
            step_index: 0,
            fixed_dt,
        })
    }

    /// Validate the complete input vector before committing any parameter.
    /// Repeated names apply in source order, matching the adaptive FMI host.
    pub fn set_inputs(&mut self, inputs: &[(&str, f64)]) -> Result<(), SimulationDiagnosticError> {
        let mut writes = Vec::with_capacity(inputs.len());
        for &(name, value) in inputs {
            if !value.is_finite() {
                return Err(SimulationDiagnosticError::Solver(format!(
                    "input '{name}' must be finite, got {value}"
                )));
            }
            let index = self
                .runtime
                .model
                .problem
                .solve_layout
                .input_parameter_index(name)
                .ok_or_else(|| {
                    SimulationDiagnosticError::Solver(format!("unknown input '{name}'"))
                })?;
            if index >= self.params.len() {
                return Err(SimulationDiagnosticError::Solver(format!(
                    "input '{name}' maps to parameter slot {index}, outside the {}-slot parameter vector",
                    self.params.len(),
                )));
            }
            writes.push((index, value));
        }
        for (index, value) in writes {
            self.params[index] = value;
        }
        Ok(())
    }

    /// Advance by exactly one configured fixed step. A different step size is
    /// rejected rather than rounded or subdivided, preserving the prediction
    /// contract at the call boundary.
    pub fn step(&mut self, dt: f64) -> Result<(), SimulationDiagnosticError> {
        if dt.to_bits() != self.fixed_dt.to_bits() {
            return Err(SimulationDiagnosticError::Solver(format!(
                "fixed-rk4 received dt={dt:?}, but its configured step is {:?}",
                self.fixed_dt,
            )));
        }
        self.rk4_step()
    }

    pub fn time(&self) -> f64 {
        self.time_origin + self.step_index as f64 * self.fixed_dt
    }

    pub fn state(&self) -> Result<SessionState, SimulationDiagnosticError> {
        Ok(SessionState {
            time: self.time(),
            values: self.visible_values()?,
        })
    }

    pub fn input_names(&self) -> &[String] {
        self.runtime.model.problem.solve_layout.input_scalar_names()
    }

    fn rk4_step(&mut self) -> Result<(), SimulationDiagnosticError> {
        let h = self.fixed_dt;
        let t = self.time();
        let y0 = self.state.clone();
        let k1 = self.derivatives(t, &y0)?;
        let y2 = stage_state(&y0, &k1, h * 0.5);
        let k2 = self.derivatives(t + h * 0.5, &y2)?;
        let y3 = stage_state(&y0, &k2, h * 0.5);
        let k3 = self.derivatives(t + h * 0.5, &y3)?;
        let y4 = stage_state(&y0, &k3, h);
        let k4 = self.derivatives(t + h, &y4)?;
        for (state, (((a, b), c), d)) in self
            .state
            .iter_mut()
            .zip(k1.iter().zip(&k2).zip(&k3).zip(&k4))
        {
            *state += (h / 6.0) * (*a + 2.0 * *b + 2.0 * *c + *d);
            if !state.is_finite() {
                return Err(SimulationDiagnosticError::Solver(
                    "fixed-rk4 produced a non-finite state".into(),
                ));
            }
        }
        self.step_index = self.step_index.checked_add(1).ok_or_else(|| {
            SimulationDiagnosticError::Solver("fixed-rk4 step index overflowed".into())
        })?;
        Ok(())
    }

    fn derivatives(&self, time: f64, state: &[f64]) -> Result<Vec<f64>, SimulationDiagnosticError> {
        self.runtime
            .eval_state_derivatives(
                time,
                state,
                &self.params,
                ALGEBRAIC_REFRESH_TOLERANCE,
                ALGEBRAIC_REFRESH_MAX_ITERS,
            )
            .map_err(|error| SimulationDiagnosticError::Solver(error.to_string()))
    }

    fn visible_values(&self) -> Result<IndexMap<String, f64>, SimulationDiagnosticError> {
        let solver_y = self
            .runtime
            .full_solver_y(
                self.time(),
                &self.state,
                &self.params,
                ALGEBRAIC_REFRESH_TOLERANCE,
                ALGEBRAIC_REFRESH_MAX_ITERS,
            )
            .map_err(|error| SimulationDiagnosticError::Solver(error.to_string()))?;
        self.runtime
            .visible_values_for_names(
                &solver_y,
                &self.params,
                self.time(),
                &self.runtime.model.visible_names,
            )
            .map_err(|error| SimulationDiagnosticError::Solver(error.to_string()))
    }
}

fn stage_state(state: &[f64], derivative: &[f64], scale: f64) -> Vec<f64> {
    state
        .iter()
        .zip(derivative)
        .map(|(value, derivative)| *value + scale * *derivative)
        .collect()
}

fn reject_unsupported_constructs(
    model: &rumoca_ir_solve::SolveModel,
) -> Result<(), SimulationDiagnosticError> {
    let problem = &model.problem;
    let unsupported = [
        (!problem.events.root_conditions.is_empty(), "root events"),
        (
            !problem.events.scheduled_root_conditions.is_empty(),
            "scheduled root events",
        ),
        (
            !problem.events.scheduled_time_events.is_empty(),
            "scheduled time events",
        ),
        (
            !problem.events.dynamic_time_event_names.is_empty()
                || !problem.events.dynamic_time_event_rhs.is_empty(),
            "dynamic time events",
        ),
        (
            !problem.events.action_conditions.is_empty(),
            "event actions",
        ),
        (!problem.events.actions.is_empty(), "event actions"),
        (
            !problem.discrete.runtime_assignment_rhs.is_empty()
                || !problem.discrete.runtime_assignment_targets.is_empty()
                || !problem.discrete.rhs.is_empty()
                || !problem.discrete.update_targets.is_empty()
                || !problem.discrete.pre_modes.is_empty()
                || !problem.discrete.observation_refresh.is_empty(),
            "discrete assignments",
        ),
        (
            !problem.clocks.periodic_event_schedules.is_empty(),
            "clock schedules",
        ),
        (!model.external_tables.is_empty(), "external tables"),
    ];
    let unsupported = unsupported
        .into_iter()
        .filter_map(|(present, name)| present.then_some(name))
        .collect::<Vec<_>>();
    if unsupported.is_empty() {
        return Ok(());
    }
    Err(SimulationDiagnosticError::Solver(format!(
        "fixed-rk4 only supports continuous event-free models; unsupported constructs: {}",
        unsupported.join(", "),
    )))
}
