//! Live solver selection over checked Rumoca FMI components.

use rumoca_compile::compile::Dae;
use rumoca_core::VarName;
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
    /// Admit the complete communication-point input vector atomically.
    pub fn set_inputs(&mut self, inputs: &[(&str, f64)]) -> Result<(), SimulationDiagnosticError> {
        match self {
            Self::Adaptive(session) => session.set_inputs(inputs).map_err(adaptive_error),
            Self::Fixed(session) => session.set_inputs(inputs),
        }
    }

    /// Reach the master's communication point using the selected backend's
    /// internal integration policy.
    pub fn advance_to(&mut self, target_time: f64) -> Result<(), SimulationDiagnosticError> {
        match self {
            Self::Adaptive(session) => session.advance_to(target_time).map_err(adaptive_error),
            Self::Fixed(session) => session.advance_to(target_time),
        }
    }

    /// Collect the complete host observation with shared canonical variable names.
    pub fn observables(&self) -> Result<Vec<(VarName, f64)>, SimulationDiagnosticError> {
        let output_count = match self {
            Self::Adaptive(session) => session.variable_names().len(),
            Self::Fixed(session) => session.output_count(),
        };
        let mut values = Vec::with_capacity(output_count + self.input_names().len());
        let visit = |name: &VarName, value| values.push((name.clone(), value));
        match self {
            Self::Adaptive(session) => session.visit_visible_values(visit).map_err(adaptive_error),
            Self::Fixed(session) => session.visit_visible_values(visit),
        }?;
        let time = self.time();
        if !time.is_finite() {
            return Err(SimulationDiagnosticError::Solver(format!(
                "observation time is not finite: {}",
                time
            )));
        }
        if let Some((name, value)) = values.iter().find(|(_, value)| !value.is_finite()) {
            return Err(SimulationDiagnosticError::Solver(format!(
                "observation '{name}' is not finite ({value}) at t={}",
                time
            )));
        }
        Ok(values)
    }

    pub fn time(&self) -> f64 {
        match self {
            Self::Adaptive(session) => session.time(),
            Self::Fixed(session) => session.time(),
        }
    }

    pub fn input_names(&self) -> &[String] {
        match self {
            Self::Adaptive(session) => session.input_names(),
            Self::Fixed(session) => session.input_names(),
        }
    }

    /// Parameter values belong to the initialized common host, independent of
    /// whether the numerical backend uses fixed or adaptive steps.
    pub fn parameter_values(&self) -> Result<Vec<(String, f64)>, SimulationDiagnosticError> {
        match self {
            Self::Adaptive(session) => session
                .parameter_values()
                .map(|values| values.into_iter().collect())
                .map_err(adaptive_error),
            Self::Fixed(session) => session.parameter_values(),
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
        "fixedrk4" => FixedStepSession::from_fmi_component(component, options)
            .map(|session| LiveStepper::Fixed(Box::new(session))),
        "rk45" => rumoca_sim::rk45::SimulationSession::from_fmi_component(component, options)
            .map(|session| LiveStepper::Adaptive(Box::new(session))),
        _ => Err(SimulationDiagnosticError::Solver(format!(
            "solver '{}' has no admitted live session constructor",
            spec.id.as_str()
        ))),
    }
}
