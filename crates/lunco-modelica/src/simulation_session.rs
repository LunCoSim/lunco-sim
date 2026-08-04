//! One construction boundary for rumoca simulation sessions.

use rumoca_compile::compile::Dae;
use rumoca_sim::{SimOptions, SimulationDiagnosticError, SimulationSession};

use crate::fixed_step::FixedStepSession;
use lunco_experiments::solver::{SolverId, SolverSpec};

/// Live Modelica stepper selected by the authoritative solver capability.
pub enum LiveStepper {
    Adaptive(SimulationSession),
    Fixed(FixedStepSession),
}

impl LiveStepper {
    pub fn set_input(&mut self, name: &str, value: f64) -> Result<(), SimulationDiagnosticError> {
        match self {
            Self::Adaptive(session) => session.set_input(name, value),
            Self::Fixed(session) => session.set_input(name, value),
        }
    }

    pub fn reset(&mut self, t_start: f64) -> Result<(), SimulationDiagnosticError> {
        match self {
            Self::Adaptive(session) => session.reset(t_start),
            Self::Fixed(session) => session.reset(t_start),
        }
    }

    pub fn step(&mut self, dt: f64) -> Result<(), SimulationDiagnosticError> {
        match self {
            Self::Adaptive(session) => session.step(dt),
            Self::Fixed(session) => session.step(dt),
        }
    }

    pub fn state(&self) -> Result<rumoca_sim::SessionState, SimulationDiagnosticError> {
        match self {
            Self::Adaptive(session) => session.state(),
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
            Self::Adaptive(session) => session.get(name),
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
    if spec.id == SolverId::from("fixedrk4") {
        FixedStepSession::new(dae, options).map(LiveStepper::Fixed)
    } else {
        construct(dae, options).map(LiveStepper::Adaptive)
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
