//! Live solver selection over checked Rumoca FMI components.

use rumoca_compile::compile::Dae;
use rumoca_ir_solve::fmi::FmiComponent;
use rumoca_sim::{SimOptions, SimulationDiagnosticError};

use crate::fixed_step::FixedStepSession;
use lunco_experiments::solver::SolverSpec;

/// Live Modelica stepper selected by the authoritative solver capability.
pub enum LiveStepper {
    Adaptive(Box<rumoca_sim::rk45::SimulationSession>),
    Fixed(Box<FixedStepSession>),
}

fn adaptive_error(error: rumoca_sim::SimError) -> SimulationDiagnosticError {
    SimulationDiagnosticError::Solver(error.to_string())
}

impl LiveStepper {
    pub fn set_input(&mut self, name: &str, value: f64) -> Result<(), SimulationDiagnosticError> {
        match self {
            Self::Adaptive(session) => session.set_input(name, value).map_err(adaptive_error),
            Self::Fixed(session) => session.set_input(name, value),
        }
    }

    pub fn step(&mut self, dt: f64) -> Result<(), SimulationDiagnosticError> {
        match self {
            Self::Adaptive(session) => session.step(dt).map_err(adaptive_error),
            Self::Fixed(session) => session.step(dt),
        }
    }

    pub fn state(&self) -> Result<rumoca_sim::SessionState, SimulationDiagnosticError> {
        let state = match self {
            Self::Adaptive(session) => session
                .state()
                .map(|state| rumoca_sim::SessionState {
                    time: state.time,
                    values: state.values,
                })
                .map_err(adaptive_error),
            Self::Fixed(session) => session.state(),
        }?;
        if !state.time.is_finite() {
            return Err(SimulationDiagnosticError::Solver(format!(
                "observation time is not finite: {}",
                state.time
            )));
        }
        if let Some((name, value)) = state.values.iter().find(|(_, value)| !value.is_finite()) {
            return Err(SimulationDiagnosticError::Solver(format!(
                "observation '{name}' is not finite ({value}) at t={}",
                state.time
            )));
        }
        Ok(state)
    }

    pub fn time(&self) -> f64 {
        match self {
            Self::Adaptive(session) => session.time(),
            Self::Fixed(session) => session.time(),
        }
    }

    pub fn get(&self, name: &str) -> Result<Option<f64>, SimulationDiagnosticError> {
        match self {
            Self::Adaptive(session) => session.get(name).map_err(adaptive_error),
            Self::Fixed(session) => session.get(name),
        }
    }

    pub fn input_names(&self) -> &[String] {
        match self {
            Self::Adaptive(session) => session.input_names(),
            Self::Fixed(session) => session.input_names(),
        }
    }
}

/// Build the real-time co-simulation session.
///
/// This boundary selects the registered live backend. Rumoca owns checked
/// preparation and adaptive session construction.
pub fn live(
    dae: &Dae,
    spec: &SolverSpec,
    options: SimOptions,
) -> Result<LiveStepper, SimulationDiagnosticError> {
    let component = rumoca_sim::prepare_fmi_component(dae, &options)?;
    live_from_fmi_component(&component, spec, options)
}

/// Build a live session from a checked FMI component. This keeps solver construction
/// separate from DAE lowering, which is the expensive and reusable part.
pub fn live_from_fmi_component(
    component: &FmiComponent,
    spec: &SolverSpec,
    options: SimOptions,
) -> Result<LiveStepper, SimulationDiagnosticError> {
    match spec.id.as_str() {
        "fixedrk4" => {
            FixedStepSession::from_continuous_model(component.runtime_view().model(), options, None)
                .map(|session| LiveStepper::Fixed(Box::new(session)))
        }
        "rk45" => rumoca_sim::rk45::SimulationSession::from_fmi_component(component, options)
            .map(|session| LiveStepper::Adaptive(Box::new(session))),
        _ => Err(SimulationDiagnosticError::Solver(format!(
            "solver '{}' has no admitted live session constructor",
            spec.id.as_str()
        ))),
    }
}
