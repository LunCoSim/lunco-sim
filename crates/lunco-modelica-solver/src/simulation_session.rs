//! One construction boundary for rumoca simulation sessions.

use rumoca_compile::compile::Dae;
use rumoca_ir_solve::fmi::FmiComponent;
use rumoca_sim::{SimOptions, SimulationDiagnosticError, SimulationSession};

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

    pub fn reset(&mut self, t_start: f64) -> Result<(), SimulationDiagnosticError> {
        match self {
            Self::Adaptive(session) => session.reset(t_start).map_err(adaptive_error),
            Self::Fixed(session) => session.reset(t_start),
        }
    }

    pub fn step(&mut self, dt: f64) -> Result<(), SimulationDiagnosticError> {
        match self {
            Self::Adaptive(session) => session.step(dt).map_err(adaptive_error),
            Self::Fixed(session) => session.step(dt),
        }
    }

    pub fn state(&self) -> Result<rumoca_sim::SessionState, SimulationDiagnosticError> {
        match self {
            Self::Adaptive(session) => session
                .state()
                .map(|state| rumoca_sim::SessionState {
                    time: state.time,
                    values: state.values,
                })
                .map_err(adaptive_error),
            Self::Fixed(session) => session.state(),
        }
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
/// The worker owns the fixed-step live solver policy; this boundary owns the
/// rumoca construction so the live path cannot grow a second constructor.
pub fn live(
    dae: &Dae,
    spec: &SolverSpec,
    options: SimOptions,
) -> Result<LiveStepper, SimulationDiagnosticError> {
    let component = lower_for_live(dae, &options)?;
    live_from_fmi_component(&component, spec, options)
}

/// Lower a compiled DAE once at the live-session boundary. Callers that own a
/// worker-local prepared-model cache use this function directly and then pass
/// the cached result to [`live_from_fmi_component`].
pub fn lower_for_live(
    dae: &Dae,
    options: &SimOptions,
) -> Result<FmiComponent, SimulationDiagnosticError> {
    rumoca_sim::prepare_fmi_component(dae, options)
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

/// Build an interactive workbench/experiment session.
pub fn interactive(
    dae: &Dae,
    options: SimOptions,
) -> Result<SimulationSession, SimulationDiagnosticError> {
    construct(dae, options)
}

/// Build an explicit command-line or diagnostic-probe session.
pub fn cli(dae: &Dae, options: SimOptions) -> Result<SimulationSession, SimulationDiagnosticError> {
    construct(dae, options)
}

/// The sole production construction of a rumoca simulation session.
fn construct(
    dae: &Dae,
    options: SimOptions,
) -> Result<SimulationSession, SimulationDiagnosticError> {
    SimulationSession::new(dae, options)
}
