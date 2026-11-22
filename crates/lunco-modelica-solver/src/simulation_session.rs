//! LunCo solver selection over Rumoca's public simulation-session API.

use rumoca_core::VarName;
use rumoca_ir_dae::{Dae, ScalarType, VariableRole};
use rumoca_sim::{SimOptions, SimulationDiagnosticError, SimulationSession};
use std::collections::HashMap;

enum LiveSession {
    Adaptive(SimulationSession),
    Fixed(crate::fixed_step::FixedStepSession),
}

/// LunCo convenience lookup for a requested subset of Rumoca session values.
/// Rumoca exposes the complete checked session state; name selection belongs
/// to this application adapter.
pub trait SelectedValuesExt {
    fn selected_values(
        &self,
        names: &[String],
    ) -> Result<Vec<Option<f64>>, SimulationDiagnosticError>;
}

impl SelectedValuesExt for SimulationSession {
    fn selected_values(
        &self,
        names: &[String],
    ) -> Result<Vec<Option<f64>>, SimulationDiagnosticError> {
        let state = self.state()?;
        Ok(names
            .iter()
            .map(|name| state.values.get(name).copied())
            .collect())
    }
}

/// Live Modelica session selected by LunCo's registered solver capability.
pub struct LiveStepper {
    session: LiveSession,
    parameter_values: Vec<(String, f64)>,
}

impl LiveStepper {
    /// Admit the complete communication-point input vector atomically.
    pub fn set_inputs(&mut self, inputs: &[(&str, f64)]) -> Result<(), SimulationDiagnosticError> {
        match &mut self.session {
            LiveSession::Adaptive(session) => session.set_inputs(inputs),
            LiveSession::Fixed(session) => session.set_inputs(inputs),
        }
    }

    /// Reach the master's communication point using Rumoca's solver policy.
    pub fn advance_to(&mut self, target_time: f64) -> Result<(), SimulationDiagnosticError> {
        match &mut self.session {
            LiveSession::Adaptive(session) => session.advance_to(target_time),
            LiveSession::Fixed(session) => session.advance_to(target_time),
        }
    }

    /// Collect the visible host observation with canonical variable names.
    pub fn observables(&self) -> Result<Vec<(VarName, f64)>, SimulationDiagnosticError> {
        let time = self.time();
        if !time.is_finite() {
            return Err(SimulationDiagnosticError::Solver(format!(
                "observation time is not finite: {time}"
            )));
        }
        let mut values = self.visible_values()?;
        for (name, value) in &self.parameter_values {
            if let Some((_, current)) = values.iter_mut().find(|(current, _)| current == name) {
                *current = *value;
            } else {
                values.push((name.clone(), *value));
            }
        }
        values
            .into_iter()
            .map(|(name, value)| {
                if !value.is_finite() {
                    return Err(SimulationDiagnosticError::Solver(format!(
                        "observation '{name}' is not finite ({value}) at t={time}"
                    )));
                }
                Ok((VarName::from(name), value))
            })
            .collect()
    }

    pub fn time(&self) -> f64 {
        match &self.session {
            LiveSession::Adaptive(session) => session.time(),
            LiveSession::Fixed(session) => session.time(),
        }
    }

    pub fn input_names(&self) -> &[String] {
        match &self.session {
            LiveSession::Adaptive(session) => session.input_names(),
            LiveSession::Fixed(session) => session.input_names(),
        }
    }

    pub fn get(&self, name: &str) -> Result<Option<f64>, SimulationDiagnosticError> {
        match &self.session {
            LiveSession::Adaptive(session) => session.get(name),
            LiveSession::Fixed(session) => Ok(session
                .visible_values()?
                .into_iter()
                .find_map(|(visible, value)| (visible == name).then_some(value))
                .or_else(|| {
                    self.parameter_values
                        .iter()
                        .find_map(|(parameter, value)| (parameter == name).then_some(*value))
                })),
        }
    }

    pub fn selected_values(
        &self,
        names: &[String],
    ) -> Result<Vec<Option<f64>>, SimulationDiagnosticError> {
        match &self.session {
            LiveSession::Adaptive(session) => session.selected_values(names),
            LiveSession::Fixed(session) => {
                let values = session.visible_values()?;
                Ok(names
                    .iter()
                    .map(|name| {
                        values
                            .iter()
                            .find_map(|(visible, value)| (visible == name).then_some(*value))
                            .or_else(|| {
                                self.parameter_values.iter().find_map(|(parameter, value)| {
                                    (parameter == name).then_some(*value)
                                })
                            })
                    })
                    .collect())
            }
        }
    }

    /// Initialized numeric parameter coordinates, including dependent values.
    pub fn parameter_values(&self) -> &[(String, f64)] {
        &self.parameter_values
    }
}

/// Build a live session through Rumoca's authoritative solver constructor.
pub fn live(dae: &Dae, options: SimOptions) -> Result<LiveStepper, SimulationDiagnosticError> {
    let parameter_values = parameter_values(dae, &options)?;
    let session = SimulationSession::new_with_diagnostics(dae, options)?;
    Ok(LiveStepper {
        session: LiveSession::Adaptive(session),
        parameter_values,
    })
}

/// Construct the session implementation selected by LunCo's solver registry.
pub fn live_for_solver(
    dae: &Dae,
    spec: &lunco_experiments::solver::SolverSpec,
    options: SimOptions,
) -> Result<LiveStepper, SimulationDiagnosticError> {
    match spec.id.as_str() {
        "fixedrk4" => {
            if !spec.caps.fixed_step || !spec.caps.deterministic {
                return Err(SimulationDiagnosticError::Solver(
                    "fixed-rk4 must declare fixed-step deterministic capability".into(),
                ));
            }
            let session = crate::fixed_step::FixedStepSession::new(dae, options)?;
            let parameter_values = session.parameter_values()?;
            Ok(LiveStepper {
                session: LiveSession::Fixed(session),
                parameter_values,
            })
        }
        "bdf" | "rk45" => live(dae, options),
        _ => Err(SimulationDiagnosticError::Solver(format!(
            "registered solver '{}' has no live session adapter",
            spec.id
        ))),
    }
}

impl LiveStepper {
    fn visible_values(&self) -> Result<Vec<(String, f64)>, SimulationDiagnosticError> {
        match &self.session {
            LiveSession::Adaptive(session) => Ok(session.state()?.values.into_iter().collect()),
            LiveSession::Fixed(session) => session.visible_values(),
        }
    }
}

fn parameter_has_numeric_projection(scalar_type: ScalarType) -> bool {
    matches!(
        scalar_type,
        ScalarType::Real | ScalarType::Integer | ScalarType::Enumeration | ScalarType::Boolean
    )
}

fn parameter_values(
    dae: &Dae,
    options: &SimOptions,
) -> Result<Vec<(String, f64)>, SimulationDiagnosticError> {
    let overrides = options
        .param_overrides
        .iter()
        .map(|(name, value)| (name.as_str(), *value))
        .collect::<HashMap<_, _>>();
    dae.inspect(|view| {
        let mut evaluator =
            rumoca_eval_dae::NumericEvaluator::with_overrides(view, |variable, scalar| {
                (variable.role() == VariableRole::Parameter && variable.is_tunable())
                    .then(|| {
                        variable
                            .scalar_name(scalar)
                            .and_then(|name| overrides.get(name.as_str()).copied())
                    })
                    .flatten()
            });
        let mut values = Vec::new();
        for (id, variable) in view.variables().filter(|(_, variable)| {
            variable.role() == VariableRole::Parameter
                && parameter_has_numeric_projection(variable.value_type().scalar_type())
        }) {
            let names = variable.scalar_names().collect::<Vec<_>>();
            let evaluated = evaluator
                .initial_value(id)
                .map_err(|error| SimulationDiagnosticError::Solver(error.to_string()))?;
            if names.len() != evaluated.len() {
                return Err(SimulationDiagnosticError::Solver(format!(
                    "parameter '{}' has {} scalar names but {} initialized values",
                    variable.name(),
                    names.len(),
                    evaluated.len()
                )));
            }
            values.extend(names.into_iter().zip(evaluated));
        }
        if let Some((name, value)) = values.iter().find(|(_, value)| !value.is_finite()) {
            return Err(SimulationDiagnosticError::Solver(format!(
                "initialized parameter '{name}' is not finite ({value})"
            )));
        }
        Ok(values)
    })
}
