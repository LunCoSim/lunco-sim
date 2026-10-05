use indexmap::IndexMap;
use rumoca_solver::RuntimeSolveError;
use std::hash::Hash;

use crate::refresh_plan::AlgebraicRefreshRow;

/// Write the Newton iterate back into the plan rows' target slots.
pub(super) fn write_refresh_targets(rows: &[AlgebraicRefreshRow], x: &[f64], solver_y: &mut [f64]) {
    for (row, value) in rows.iter().zip(x) {
        solver_y[row.target_index] = *value;
    }
}

/// Borrowed inputs for one Newton Jacobian assembly.
#[derive(Clone, Copy)]
pub(super) struct NewtonProbe<'a> {
    pub(super) rows: &'a [AlgebraicRefreshRow],
    pub(super) x: &'a [f64],
    pub(super) f_base: &'a [f64],
    pub(super) residual: &'a [f64],
    pub(super) t: f64,
    pub(super) params: &'a [f64],
}

/// Apply the eliminated Newton steps; false when any step is non-finite.
pub(super) fn apply_newton_steps(
    x: &mut [f64],
    augmented: &crate::linear_solve::AugmentedMatrix,
    fraction: f64,
) -> bool {
    let m = x.len();
    for (j, value) in x.iter_mut().enumerate() {
        let step = augmented.get(j, m);
        if !step.is_finite() {
            return false;
        }
        *value += fraction * step;
        if !value.is_finite() {
            return false;
        }
    }
    true
}

/// Preserve the requested absolute tolerance, with a sixteen-ULP floor for
/// accumulated evaluation/subtraction roundoff in compound rows. A traced
/// power/photometry row retained eight ULPs after Newton settled; twice that
/// bounds numerical noise without a relative engineering tolerance.
pub(super) fn refresh_difference_converged(difference: f64, value: f64, tol: f64) -> bool {
    let magnitude = value.abs();
    let roundoff = 16.0 * (magnitude.next_up() - magnitude);
    difference.is_finite()
        && value.is_finite()
        && (difference.abs() <= tol || (roundoff.is_finite() && difference.abs() <= roundoff))
}

pub(super) fn zero_runtime_values(
    len: usize,
    context: &'static str,
) -> Result<Vec<f64>, RuntimeSolveError> {
    let mut values = Vec::new();
    reserve_runtime_vec_capacity(&mut values, len, context)?;
    values.resize(len, 0.0);
    Ok(values)
}

pub(super) fn copy_runtime_values(
    values: &[f64],
    context: &'static str,
) -> Result<Vec<f64>, RuntimeSolveError> {
    let mut copy = Vec::new();
    reserve_runtime_vec_capacity(&mut copy, values.len(), context)?;
    copy.extend_from_slice(values);
    Ok(copy)
}

pub(super) fn copy_runtime_values_into(
    dst: &mut Vec<f64>,
    values: &[f64],
    context: &'static str,
) -> Result<(), RuntimeSolveError> {
    if dst.len() < values.len() {
        reserve_runtime_vec_capacity(dst, values.len() - dst.len(), context)?;
    }
    dst.clear();
    dst.extend_from_slice(values);
    Ok(())
}

pub(super) fn resize_runtime_values(
    values: &mut Vec<f64>,
    len: usize,
    value: f64,
    context: &'static str,
) -> Result<(), RuntimeSolveError> {
    if values.len() < len {
        reserve_runtime_vec_capacity(values, len - values.len(), context)?;
    }
    values.resize(len, value);
    Ok(())
}

pub(super) fn reserve_runtime_vec_capacity<T>(
    values: &mut Vec<T>,
    capacity: usize,
    context: &'static str,
) -> Result<(), RuntimeSolveError> {
    values
        .try_reserve_exact(capacity)
        .map_err(|_| RuntimeSolveError::solve_ir(format!("{context} capacity overflows")))
}

pub(super) fn reserve_runtime_index_map_capacity<K, V>(
    values: &mut IndexMap<K, V>,
    capacity: usize,
    context: &'static str,
) -> Result<(), RuntimeSolveError>
where
    K: Eq + Hash,
{
    values
        .try_reserve(capacity)
        .map_err(|_| RuntimeSolveError::solve_ir(format!("{context} capacity overflows")))
}
