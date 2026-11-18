//! Fixed-step RK4 integration on Rumoca's shared Model Exchange host.
//!
//! The host owns initialization, input transactions, component evaluation and
//! observations. Rumoca's RK4 plugin advances the continuous state on the
//! communication lattice selected by the prediction host.

use rumoca_compile::compile::Dae;
use rumoca_core::VarName;
use rumoca_ir_solve::fmi::FmiComponent;
use rumoca_sim::{SimOptions, SimulationDiagnosticError};
use rumoca_solver::fmi_me::{
    MeInstanceConfig, MeIntegratorBackend, MeModelSource,
    driver::{advance_live_session, live_session_options},
    fixed_step::FixedRk4Integrator,
    integrator::canonical_coordinate,
    session::{MeRetainedComponent, MeSimulationSession},
};

/// A continuous fixed-step session with parameter-only event partitions.
pub struct FixedStepSession {
    session: MeSimulationSession<'static, 'static>,
    output_count: usize,
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
        validate_fixed_step_component(component)?;
        let config = MeInstanceConfig::new(
            "luncosim-fixed-rk4",
            options.rtol,
            options.t_start,
            options.t_end,
        )
        .map_err(solver_error)?;
        let execution = rumoca_sim::fmi_execution_backend(component, &options)?;
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
            Some(Box::new(
                FixedRk4Integrator::new(options.t_start, fixed_dt).map_err(solver_error)?,
            ))
        };
        let session = host.into_session(plugin).map_err(solver_error)?;
        let output_count = session.output_names().map_err(solver_error)?.len();
        Ok(Self {
            session,
            output_count,
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
    pub fn visit_visible_values(
        &self,
        visit: impl FnMut(&VarName, f64),
    ) -> Result<(), SimulationDiagnosticError> {
        self.session
            .visit_visible_values(visit)
            .map_err(solver_error)
    }
    pub fn input_names(&self) -> &[String] {
        self.session.input_names()
    }
    /// Initialized parameters read by the shared Model Exchange host.
    pub fn parameter_values(&self) -> Result<Vec<(String, f64)>, SimulationDiagnosticError> {
        self.session
            .parameter_values()
            .map(|values| values.into_iter().collect())
            .map_err(solver_error)
    }

    pub(crate) fn output_count(&self) -> usize {
        self.output_count
    }
}

fn validate_fixed_step_component(
    component: &FmiComponent,
) -> Result<(), SimulationDiagnosticError> {
    if !component.runtime_view().model().external_tables.is_empty() {
        return Err(solver_error("fixed-rk4 does not support external tables"));
    }
    component
        .runtime_view()
        .validate_parameter_only_events()
        .map_err(|error| solver_error(format!(
            "fixed-rk4 cannot start this model: {error}. Its event partitions must depend only on parameters and constants"
        )))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod observation_diagnostics {
    use super::*;
    use std::{hint::black_box, time::Instant};

    /// Compare complete observation collection through the existing public ME
    /// surfaces. Construction and communication-point advancement are excluded.
    #[test]
    #[ignore = "manual observation collection timing diagnostic"]
    fn named_and_visited_observation_collection() {
        let mut compiler = rumoca_compile::Session::default();
        compiler
            .add_document(
                "observation_probe.mo",
                r#"
model ObservationProbe
  input Real command = 0;
  parameter Real gain = 2;
  output Real observed[2304];
equation
  for i in 1:2304 loop
    observed[i] = gain * command + i * time;
  end for;
end ObservationProbe;
"#,
            )
            .unwrap();
        let compiled = compiler
            .compile_model_dae_strict_reachable_uncached_with_recovery("ObservationProbe")
            .unwrap();
        let mut stepper = FixedStepSession::new(
            &compiled.dae,
            SimOptions {
                dt: Some(0.01),
                t_end: 1.0,
                ..Default::default()
            },
        )
        .unwrap();
        let labels = stepper.session.output_names().unwrap();
        assert_eq!(labels.len(), 2305);
        assert!(labels.iter().any(|name| name == "command"));
        let named = |stepper: &FixedStepSession| {
            assert!(stepper.time().is_finite());
            let values = stepper.session.visible_values().unwrap();
            assert!(values.values().all(|value| value.is_finite()));
            values
                .into_iter()
                .filter(|(name, _)| name != "time")
                .collect::<Vec<_>>()
        };
        let visited = |stepper: &FixedStepSession| {
            assert!(stepper.time().is_finite());
            let mut values = Vec::with_capacity(labels.len());
            stepper
                .visit_visible_values(|name, value| {
                    assert!(value.is_finite());
                    if name.as_str() != "time" {
                        values.push((name.to_owned(), value));
                    }
                })
                .unwrap();
            values
        };
        for pair in 0..5 {
            let command = (f64::from(pair) - 2.0) * 0.125;
            stepper.set_inputs(&[("command", command)]).unwrap();
            stepper.step(0.01).unwrap();
            let expected = named(&stepper);
            let actual = visited(&stepper);
            assert_eq!(expected.len(), actual.len());
            for ((name, value), (label, sample)) in expected.iter().zip(&actual) {
                assert_eq!(name.as_str(), label.as_str());
                assert_eq!(value.to_bits(), sample.to_bits());
            }
            let mut timings = [0_u128; 2];
            for phase in 0..2 {
                let mode = (pair as usize + phase) % 2;
                let start = Instant::now();
                for _ in 0..512 {
                    if mode == 0 {
                        drop(black_box(named(&stepper)));
                    } else {
                        drop(black_box(visited(&stepper)));
                    }
                }
                timings[mode] = start.elapsed().as_nanos();
            }
            println!(
                "OBSERVATION_COLLECTION pair={pair} named_ns={} visited_ns={} width={} command_bits={} time_bits={} last_bits={}",
                timings[0],
                timings[1],
                labels.len(),
                command.to_bits(),
                stepper.time().to_bits(),
                actual.last().unwrap().1.to_bits()
            );
        }
    }
}
