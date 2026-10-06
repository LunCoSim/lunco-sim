//! Scoped admission for a simulation's retained scalar output storage.
use std::{cell::RefCell, marker::PhantomData, rc::Rc};

use super::solve_ops::RuntimeSolveError;

/// Scalar values, including the separate recorded time vector.
#[derive(Clone, Copy, Debug)]
pub struct SolverOutputBudget {
    max_values: usize,
}
impl SolverOutputBudget {
    pub fn new(max_values: usize) -> Result<Self, RuntimeSolveError> {
        if max_values == 0 {
            return Err(RuntimeSolveError::solve_ir(
                "solver output budget must be positive",
            ));
        }
        Ok(Self { max_values })
    }

    pub fn validate_dimensions(
        self,
        outputs: usize,
        samples: usize,
    ) -> Result<(), RuntimeSolveError> {
        let values = outputs
            .checked_add(1)
            .and_then(|columns| columns.checked_mul(samples))
            .ok_or_else(|| RuntimeSolveError::solve_ir("solver output dimensions overflow"))?;
        if values > self.max_values {
            return Err(RuntimeSolveError::solve_ir(format!(
                "solver output requires {values} scalar values; configured output budget is {}",
                self.max_values
            )));
        }
        Ok(())
    }
    pub fn sample_capacity(self, outputs: usize) -> Result<usize, RuntimeSolveError> {
        let columns = outputs
            .checked_add(1)
            .ok_or_else(|| RuntimeSolveError::solve_ir("solver output dimensions overflow"))?;
        Ok(self.max_values / columns)
    }
}

struct OutputBudgetState {
    budget: SolverOutputBudget,
    /// Zero-column recorders still append to their separate time vector.
    empty_samples: usize,
}
thread_local! {
    static OUTPUT_BUDGET: RefCell<Option<OutputBudgetState>> = const { RefCell::new(None) };
}

/// One admitted solve's immutable budget. Nested calls restore their caller;
/// this guard cannot move to a different thread or Web Worker.
/// Each admitted solve owns one output recorder. Zero-column appends use the
/// scoped sample count because their only storage is the separate time vector;
/// recorders exposing that vector validate its actual length before appending.
pub struct SolverOutputBudgetGuard {
    previous: Option<OutputBudgetState>,
    _thread: PhantomData<Rc<()>>,
}
impl SolverOutputBudgetGuard {
    pub fn install(budget: SolverOutputBudget) -> Self {
        let previous = OUTPUT_BUDGET.with(|slot| {
            slot.replace(Some(OutputBudgetState {
                budget,
                empty_samples: 0,
            }))
        });
        Self {
            previous,
            _thread: PhantomData,
        }
    }
}
impl Drop for SolverOutputBudgetGuard {
    fn drop(&mut self) {
        OUTPUT_BUDGET.with(|slot| {
            slot.replace(self.previous.take());
        });
    }
}

pub fn validate_solver_output_dimensions(
    outputs: usize,
    samples: usize,
) -> Result<(), RuntimeSolveError> {
    OUTPUT_BUDGET.with(|slot| match slot.borrow().as_ref() {
        Some(state) => state.budget.validate_dimensions(outputs, samples),
        None => Ok(()),
    })
}

pub(crate) fn admit_visible_append(data: &[Vec<f64>]) -> Result<(), RuntimeSolveError> {
    OUTPUT_BUDGET.with(|slot| {
        let mut state = slot.borrow_mut();
        let Some(state) = state.as_mut() else {
            return Ok(());
        };
        let count = data.first().map_or(state.empty_samples, Vec::len);
        let next = count
            .checked_add(1)
            .ok_or_else(|| RuntimeSolveError::solve_ir("solver sample count overflow"))?;
        state.budget.validate_dimensions(data.len(), next)?;
        if data.is_empty() {
            state.empty_samples = next;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::push_visible_values;

    #[test]
    fn recorder_budget_rejects_event_growth_and_restores_scopes() {
        let guard = SolverOutputBudgetGuard::install(SolverOutputBudget::new(6).unwrap());
        let mut data = vec![Vec::new()];
        for value in [1.0, 2.0, 3.0] {
            push_visible_values(&mut data, &[value]).unwrap();
        }
        let before = data.clone();
        assert!(
            push_visible_values(&mut data, &[4.0])
                .unwrap_err()
                .to_string()
                .contains("output budget")
        );
        assert_eq!(data, before);
        {
            let _inner = SolverOutputBudgetGuard::install(SolverOutputBudget::new(2).unwrap());
            assert!(validate_solver_output_dimensions(1, 2).is_err());
        }
        assert!(validate_solver_output_dimensions(1, 3).is_ok());
        std::thread::spawn(|| assert!(validate_solver_output_dimensions(1, 100).is_ok()))
            .join()
            .unwrap();
        drop(guard);
        assert!(validate_solver_output_dimensions(1, 100).is_ok());
        assert!(SolverOutputBudget::new(0).is_err());
        assert!(
            SolverOutputBudget::new(usize::MAX)
                .unwrap()
                .validate_dimensions(usize::MAX, 2)
                .is_err()
        );
    }

    #[test]
    fn zero_visible_columns_still_admit_recorded_times() {
        let _guard = SolverOutputBudgetGuard::install(SolverOutputBudget::new(1).unwrap());
        push_visible_values(&mut [], &[]).unwrap();
        assert!(push_visible_values(&mut [], &[]).is_err());
    }
}
