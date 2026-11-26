//! LunCo solver selection over Rumoca's public simulation-session API.

use rumoca_core::VarName;
use rumoca_ir_dae::{Dae, ScalarType, VariableRole};
use rumoca_sim::{SimOptions, SimSolverMode, SimulationDiagnosticError, SimulationSession};
use std::cell::RefCell;
use std::collections::HashMap;

/// Live Modelica session selected by LunCo's registered solver capability.
pub struct LiveStepper {
    session: SimulationSession,
    parameter_values: Vec<(String, f64)>,
    observation_names: RefCell<Vec<VarName>>,
}

impl LiveStepper {
    /// Admit the complete communication-point input vector atomically.
    pub fn set_inputs(&mut self, inputs: &[(&str, f64)]) -> Result<(), SimulationDiagnosticError> {
        self.session.set_inputs(inputs)
    }

    /// Reach the master's communication point using Rumoca's solver policy.
    pub fn advance_to(&mut self, target_time: f64) -> Result<(), SimulationDiagnosticError> {
        self.session.advance_to(target_time)
    }

    /// Collect the visible host observation with canonical variable names.
    pub fn observables(&self) -> Result<Vec<(VarName, f64)>, SimulationDiagnosticError> {
        let time = self.time();
        if !time.is_finite() {
            return Err(SimulationDiagnosticError::Solver(format!(
                "observation time is not finite: {time}"
            )));
        }
        // Rumoca already returns the canonical, insertion-ordered value map.
        // Merge initialized parameter coordinates into that map before turning
        // it into the observation vector. Existing keys keep their position;
        // parameters absent from the solver state append in declaration order.
        let mut values = self.session.state()?.values;
        for (name, value) in &self.parameter_values {
            if let Some(current) = values.get_mut(name) {
                *current = *value;
            } else {
                values.insert(name.clone(), *value);
            }
        }
        if let Some((name, value)) = values.iter().find(|(_, value)| !value.is_finite()) {
            return Err(SimulationDiagnosticError::Solver(format!(
                "observation '{name}' is not finite ({value}) at t={time}"
            )));
        }
        // A session's ordered inventory normally stays fixed. Validate the
        // exact name layout, then retain canonical handles instead of entering
        // the process-wide VarName interner once per variable on every step.
        let mut names = self.observation_names.borrow_mut();
        if names.len() != values.len()
            || !names
                .iter()
                .zip(values.keys())
                .all(|(cached, live)| cached.as_str() == live.as_str())
        {
            *names = values.keys().map(|name| VarName::intern(name)).collect();
        }
        Ok(names.iter().cloned().zip(values.into_values()).collect())
    }

    pub fn time(&self) -> f64 {
        self.session.time()
    }

    pub fn input_names(&self) -> &[String] {
        self.session.input_names()
    }

    pub fn get(&self, name: &str) -> Result<Option<f64>, SimulationDiagnosticError> {
        Ok(self.session.get(name)?.or_else(|| {
            self.parameter_values
                .iter()
                .find_map(|(parameter, value)| (parameter == name).then_some(*value))
        }))
    }

    pub fn selected_values(
        &self,
        names: &[String],
    ) -> Result<Vec<Option<f64>>, SimulationDiagnosticError> {
        Ok(self
            .session
            .selected_values(names)?
            .into_iter()
            .zip(names)
            .map(|(value, name)| {
                value.or_else(|| {
                    self.parameter_values
                        .iter()
                        .find_map(|(parameter, value)| (parameter == name).then_some(*value))
                })
            })
            .collect())
    }

    /// Initialized numeric parameter coordinates, including dependent values.
    pub fn parameter_values(&self) -> &[(String, f64)] {
        &self.parameter_values
    }
}

/// Build a live session through Rumoca's authoritative solver constructor.
pub fn live(dae: &Dae, options: SimOptions) -> Result<LiveStepper, SimulationDiagnosticError> {
    live_with_constructor(dae, options, SimulationSession::new_with_diagnostics)
}

fn live_with_constructor(
    dae: &Dae,
    options: SimOptions,
    construct: impl FnOnce(&Dae, SimOptions) -> Result<SimulationSession, SimulationDiagnosticError>,
) -> Result<LiveStepper, SimulationDiagnosticError> {
    let parameter_values = parameter_values(dae, &options)?;
    let session = construct(dae, options)?;
    Ok(LiveStepper {
        session,
        parameter_values,
        observation_names: RefCell::new(Vec::new()),
    })
}

/// Construct the session implementation selected by LunCo's solver registry.
pub fn live_for_solver(
    dae: &Dae,
    spec: &lunco_experiments::solver::SolverSpec,
    options: SimOptions,
) -> Result<LiveStepper, SimulationDiagnosticError> {
    live_for_solver_with(dae, spec, options, SimulationSession::new_with_diagnostics)
}

/// Admit the registry contract before constructing a fresh Rumoca session.
pub fn live_for_solver_with(
    dae: &Dae,
    spec: &lunco_experiments::solver::SolverSpec,
    options: SimOptions,
    construct: impl FnOnce(&Dae, SimOptions) -> Result<SimulationSession, SimulationDiagnosticError>,
) -> Result<LiveStepper, SimulationDiagnosticError> {
    match spec.id.as_str() {
        "fixedrk4" => {
            if !spec.caps.fixed_step || !spec.caps.deterministic {
                return Err(SimulationDiagnosticError::Solver(
                    "fixed-rk4 must declare fixed-step deterministic capability".into(),
                ));
            }
            if options.solver_mode != SimSolverMode::FixedStep {
                return Err(SimulationDiagnosticError::Solver(
                    "fixed-rk4 requires Rumoca fixed-step options".into(),
                ));
            }
            live_with_constructor(dae, options, construct)
        }
        "bdf" | "rk45" => live_with_constructor(dae, options, construct),
        _ => Err(SimulationDiagnosticError::Solver(format!(
            "registered solver '{}' has no live session adapter",
            spec.id
        ))),
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
