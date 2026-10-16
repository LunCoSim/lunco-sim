//! Fixed-step RK4 integration on Rumoca's shared Model Exchange host.
//!
//! The host owns initialization, input transactions, component evaluation and
//! observations. The numerical plugin owns four RK stages at the exact fixed
//! cadence. Continuous extensions use those retained stages without evaluating
//! the component outside the host-issued derivative capability.

use rumoca_compile::compile::Dae;
use rumoca_ir_solve::fmi::FmiComponent;
use rumoca_sim::{SessionState, SimOptions, SimulationDiagnosticError};
use rumoca_solver::fmi_me::{
    MeAdvanceRequest, MeContinuousPoint, MeDerivativeHandle, MeInstanceConfig, MeIntegrationError,
    MeIntegratorBackend, MeModelSource, MeStepCandidate, accepted_interval_contains,
    admit_execution_backend,
    driver::{advance_live_session, live_session_options},
    integrator::canonical_coordinate,
    session::{MeRetainedComponent, MeSimulationSession},
};

/// A continuous, event-free session whose component lifecycle has one owner.
pub struct FixedStepSession {
    session: MeSimulationSession<'static, 'static>,
    input_names: Vec<String>,
    time_origin: f64,
    step_index: u64,
    fixed_dt: f64,
}

fn solver_error(error: impl std::fmt::Display) -> SimulationDiagnosticError {
    SimulationDiagnosticError::Solver(error.to_string())
}

impl FixedStepSession {
    pub fn new(dae: &Dae, options: SimOptions) -> Result<Self, SimulationDiagnosticError> {
        let component = rumoca_sim::prepare_fmi_component(dae, &options)?;
        Self::from_fmi_component(&component, options)
    }

    pub(crate) fn from_fmi_component(
        component: &FmiComponent,
        options: SimOptions,
    ) -> Result<Self, SimulationDiagnosticError> {
        let fixed_dt = options
            .dt
            .ok_or_else(|| solver_error("fixed-rk4 requires a fixed dt"))?;
        if !fixed_dt.is_finite() || fixed_dt <= 0.0 {
            return Err(solver_error("fixed-rk4 requires a finite positive dt"));
        }
        reject_unsupported_constructs(component.runtime_view().model())?;
        let config = MeInstanceConfig::new(
            "luncosim-fixed-rk4",
            options.rtol,
            options.t_start,
            options.t_end,
        )
        .map_err(solver_error)?;
        let execution =
            admit_execution_backend(options.execution_policy, None).map_err(solver_error)?;
        let retained =
            MeRetainedComponent::instantiate(MeModelSource::new(component), &config, execution)
                .map_err(solver_error)?;
        let host_options = live_session_options(
            &component.root_location(),
            options.t_start,
            options.rtol,
            options.atol,
            options.t_end - options.t_start,
            options.max_wall_seconds,
        )
        .map_err(solver_error)?;
        let host = retained.into_lease(host_options).map_err(solver_error)?;
        if host.is_terminated() {
            return Err(solver_error(
                "fixed-rk4 component terminated during initialization",
            ));
        }
        host.numerical_setup(Some(fixed_dt)).map_err(solver_error)?;
        let plugin: Option<Box<dyn MeIntegratorBackend>> = if host.state_count() == 0 {
            None
        } else {
            Some(Box::new(FixedRk4 {
                origin: canonical_coordinate(options.t_start),
                index: 0,
                dt: fixed_dt,
                derivatives: None,
                history: None,
            }))
        };
        let session = host.into_session(plugin).map_err(solver_error)?;
        let input_names = session.input_names();
        Ok(Self {
            session,
            input_names,
            time_origin: canonical_coordinate(options.t_start),
            step_index: 0,
            fixed_dt,
        })
    }

    pub fn set_inputs(&mut self, inputs: &[(&str, f64)]) -> Result<(), SimulationDiagnosticError> {
        self.session.set_inputs(inputs).map_err(solver_error)
    }

    pub fn step(&mut self, dt: f64) -> Result<(), SimulationDiagnosticError> {
        if dt.to_bits() != self.fixed_dt.to_bits() {
            return Err(solver_error(format!(
                "fixed-rk4 received dt={dt:?}, but its configured step is {:?}",
                self.fixed_dt
            )));
        }
        let index = self
            .step_index
            .checked_add(1)
            .ok_or_else(|| solver_error("fixed-rk4 step index overflowed"))?;
        let target = self.time_origin + index as f64 * self.fixed_dt;
        advance_live_session(&mut self.session, target).map_err(solver_error)?;
        self.step_index = index;
        Ok(())
    }

    /// Reach a communication point on the configured fixed-step lattice.
    /// Validate the complete target before applying any internal step.
    pub fn advance_to(&mut self, target_time: f64) -> Result<(), SimulationDiagnosticError> {
        let index = ((target_time - self.time_origin) / self.fixed_dt).round();
        let represented = self.time_origin + index * self.fixed_dt;
        let roundoff = 32.0
            * f64::EPSILON
            * (self.time_origin.abs() + target_time.abs() + (index * self.fixed_dt).abs())
                .max(self.fixed_dt);
        if !target_time.is_finite()
            || !index.is_finite()
            || index < self.step_index as f64
            || index >= u64::MAX as f64
            || !represented.is_finite()
            || (represented - target_time).abs() > roundoff
        {
            return Err(SimulationDiagnosticError::Solver(format!(
                "fixed-rk4 target {target_time:?} is not a forward point on its configured lattice (origin={}, step={})",
                self.time_origin, self.fixed_dt,
            )));
        }
        let target_index = index as u64;
        for _ in self.step_index..target_index {
            self.step(self.fixed_dt)?;
        }
        Ok(())
    }

    pub fn time(&self) -> f64 {
        self.session.time()
    }
    pub fn state(&self) -> Result<SessionState, SimulationDiagnosticError> {
        Ok(SessionState {
            time: self.time(),
            values: self.session.visible_values().map_err(solver_error)?,
        })
    }
    pub fn input_names(&self) -> &[String] {
        &self.input_names
    }
}

struct Rk4History {
    start: f64,
    end: f64,
    initial: Vec<f64>,
    final_state: Vec<f64>,
    stages: [Vec<f64>; 4],
}

struct FixedRk4 {
    origin: f64,
    index: u64,
    dt: f64,
    derivatives: Option<MeDerivativeHandle>,
    history: Option<Rk4History>,
}

fn integration_error(reason: impl Into<String>) -> MeIntegrationError {
    MeIntegrationError::Contract {
        reason: reason.into(),
    }
}

impl FixedRk4 {
    fn validate_coordinate(&self, point: &MeContinuousPoint) -> Result<(), MeIntegrationError> {
        let expected = self.origin + self.index as f64 * self.dt;
        if point.time().to_bits() != expected.to_bits() {
            return Err(integration_error(format!(
                "fixed-rk4 requires grid coordinate {expected}, received {}",
                point.time()
            )));
        }
        Ok(())
    }
}

impl MeIntegratorBackend for FixedRk4 {
    fn initialize(
        &mut self,
        point: &MeContinuousPoint,
        derivatives: MeDerivativeHandle,
    ) -> Result<(), MeIntegrationError> {
        self.validate_coordinate(point)?;
        self.derivatives = Some(derivatives);
        self.history = None;
        Ok(())
    }

    fn advance(
        &mut self,
        request: &MeAdvanceRequest,
    ) -> Result<MeStepCandidate, MeIntegrationError> {
        let point = request.current();
        self.validate_coordinate(point)?;
        let next = self
            .index
            .checked_add(1)
            .ok_or_else(|| integration_error("fixed-rk4 step index overflowed"))?;
        let end = self.origin + next as f64 * self.dt;
        let handle = self
            .derivatives
            .as_ref()
            .ok_or_else(|| integration_error("fixed-rk4 has no admitted derivative capability"))?;
        let eval = |time, state: &[f64]| {
            handle
                .derivatives(time, state)
                .map_err(|_| MeIntegrationError::DerivativeRefused)
        };
        let t = point.time();
        let h = self.dt;
        let y = point.states();
        let k1 = eval(t, y)?;
        let k2 = eval(t + h * 0.5, &stage_state(y, &k1, h * 0.5))?;
        let k3 = eval(t + h * 0.5, &stage_state(y, &k2, h * 0.5))?;
        let k4 = eval(end, &stage_state(y, &k3, h))?;
        let values: Vec<_> = y
            .iter()
            .enumerate()
            .map(|(i, value)| value + (h / 6.0) * (k1[i] + 2.0 * k2[i] + 2.0 * k3[i] + k4[i]))
            .collect();
        self.history = Some(Rk4History {
            start: t,
            end,
            initial: y.to_vec(),
            final_state: values.clone(),
            stages: [k1, k2, k3, k4],
        });
        self.index = next;
        // Classical RK4's cubic continuous extension has local order three.
        Ok(MeStepCandidate::new(end, values, 3))
    }

    fn sample(&self, time: f64, states: &mut [f64]) -> Result<(), MeIntegrationError> {
        let history = self
            .history
            .as_ref()
            .ok_or_else(|| integration_error("fixed-rk4 has no accepted interval to sample"))?;
        if states.len() != history.initial.len()
            || !accepted_interval_contains(history.start, history.end, time)
        {
            return Err(integration_error(
                "fixed-rk4 sample is outside its accepted interval or state width",
            ));
        }
        if time.to_bits() == history.end.to_bits() {
            states.copy_from_slice(&history.final_state);
            return Ok(());
        }
        if time.to_bits() == history.start.to_bits() {
            states.copy_from_slice(&history.initial);
            return Ok(());
        }
        let theta = (time - history.start) / self.dt;
        let square = theta * theta;
        let cube = square * theta;
        let weights = [
            theta - 1.5 * square + (2.0 / 3.0) * cube,
            square - (2.0 / 3.0) * cube,
            square - (2.0 / 3.0) * cube,
            -0.5 * square + (2.0 / 3.0) * cube,
        ];
        for (i, state) in states.iter_mut().enumerate() {
            let delta: f64 = history
                .stages
                .iter()
                .zip(weights)
                .map(|(stage, weight)| stage[i] * weight)
                .sum();
            *state = history.initial[i] + self.dt * delta;
        }
        Ok(())
    }

    fn truncate_reset(&mut self, point: &MeContinuousPoint) -> Result<(), MeIntegrationError> {
        self.validate_coordinate(point)?;
        self.history = None;
        Ok(())
    }
}

fn stage_state(state: &[f64], derivative: &[f64], scale: f64) -> Vec<f64> {
    state
        .iter()
        .zip(derivative)
        .map(|(value, derivative)| value + scale * derivative)
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
