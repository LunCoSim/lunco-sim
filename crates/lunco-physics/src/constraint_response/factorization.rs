//! Complete orthogonal response and feasible pivots for certified PSD islands.

use super::allocate_matrix;
use faer::{
    Conj, Mat, Par,
    dyn_stack::{MemBuffer, MemStack},
    linalg::{householder as hh, qr, triangular_solve},
};

#[derive(Debug)]
pub(super) struct Solution {
    pub multipliers: Vec<f64>,
    pub residual: OrthogonalResidual,
}

/// Left-null projection from the same Householder factors as the reaction.
/// Its roundoff certificate belongs to the projection, not geometric slack.
#[derive(Debug)]
pub(super) struct OrthogonalResidual {
    values: Mat<f64>,
    error_bound: f64,
}

/// Zero the orthogonal null coordinates, preserving the scaled reaction norm.
/// Both maintained QR kernels run serially and admit their workspace fallibly.
pub(super) fn minimum_norm(
    mut matrix: Mat<f64>,
    rhs: &[f64],
    rank_ceiling: usize,
) -> Result<Solution, &'static str> {
    let _span = bevy::log::debug_span!("coupled_response_factorization").entered();
    let n = rhs.len();
    if matrix.nrows() != n || matrix.ncols() != n {
        return Err("rank-revealing response dimensions are inconsistent");
    }
    for i in 0..n {
        if !rhs[i].is_finite() {
            return Err("rank-revealing response RHS is not finite");
        }
        for j in 0..n {
            if !matrix[(i, j)].is_finite() {
                return Err("rank-revealing response coefficients are not finite");
            }
        }
    }
    let mut result = Vec::new();
    result
        .try_reserve_exact(n)
        .map_err(|_| "rank-revealing response result allocation failed")?;
    result.resize(n, 0.0);
    let mut residual = OrthogonalResidual {
        values: allocate_matrix(n, 1, |_, _| 0.0)?,
        error_bound: 0.0,
    };
    if n == 0 || rhs.iter().all(|&x| x == 0.0) {
        return Ok(Solution {
            multipliers: result,
            residual,
        });
    }
    let block = qr::col_pivoting::factor::recommended_block_size::<f64>(n, n);
    let mut coefficients = allocate_matrix(block, n, |_, _| 0.0)?;
    let size = n
        .checked_mul(2)
        .ok_or("rank-revealing response permutation overflow")?;
    let mut permutations = Vec::new();
    permutations
        .try_reserve_exact(size)
        .map_err(|_| "rank-revealing response permutation allocation failed")?;
    permutations.resize(size, 0usize);
    let (forward, backward) = permutations.split_at_mut(n);
    let mut memory =
        MemBuffer::try_new(qr::col_pivoting::factor::qr_in_place_scratch::<usize, f64>(
            n,
            n,
            block,
            Par::Seq,
            Default::default(),
        ))
        .map_err(|_| "rank-revealing response QR workspace allocation failed")?;
    let (_, permutation) = qr::col_pivoting::factor::qr_in_place(
        matrix.as_mut(),
        coefficients.as_mut(),
        forward,
        backward,
        Par::Seq,
        MemStack::new(&mut memory),
        Default::default(),
    );
    let maximum = (0..n).map(|i| matrix[(i, i)].abs()).fold(0.0, f64::max);
    let cutoff = maximum * f64::EPSILON * n as f64;
    let rank = (0..n.min(rank_ceiling))
        .take_while(|&i| matrix[(i, i)].abs() > cutoff)
        .count();
    if rank == 0 {
        return Err("rank-revealing response has no correcting rank");
    }
    let mut right = allocate_matrix(n, rank, |i, j| if i < j { 0.0 } else { matrix[(j, i)] })?;
    // The copied triangular factor and Householder basis have separate owners.
    for j in 0..n {
        for i in 0..=j {
            matrix[(i, j)] = if i == j { 1.0 } else { 0.0 };
        }
    }
    let mut transformed = allocate_matrix(n, 1, |i, _| rhs[i])?;
    let rhs_norm = transformed.norm_l2();
    if !rhs_norm.is_finite() {
        return Err("rank-revealing residual cannot be bounded in f64");
    }
    let mut memory = MemBuffer::try_new(
        hh::apply_block_householder_sequence_transpose_on_the_left_in_place_scratch::<f64>(
            n, block, 1,
        ),
    )
    .map_err(|_| "rank-revealing response left workspace allocation failed")?;
    hh::apply_block_householder_sequence_transpose_on_the_left_in_place_with_conj(
        matrix.as_ref(),
        coefficients.as_ref(),
        Conj::No,
        transformed.as_mut(),
        Par::Seq,
        MemStack::new(&mut memory),
    );
    // Project the Householder tail directly. Subtracting A*x would lose small
    // null components when large internal reactions cancel.
    // Two sequences of at most n reflectors, each with a dot and an update,
    // bound the projection by eight n-squared scalar operations.
    let roundoff = 8.0 * n as f64 * n as f64 * f64::EPSILON;
    if roundoff >= 1.0 {
        return Err("rank-revealing residual cannot be bounded in f64");
    }
    residual.error_bound = roundoff / (1.0 - roundoff) * rhs_norm;
    if rank < n {
        for i in rank..n {
            residual.values[(i, 0)] = transformed[(i, 0)];
        }
        let mut memory = MemBuffer::try_new(
            hh::apply_block_householder_sequence_on_the_left_in_place_scratch::<f64>(n, block, 1),
        )
        .map_err(|_| "rank-revealing residual workspace allocation failed")?;
        hh::apply_block_householder_sequence_on_the_left_in_place_with_conj(
            matrix.as_ref(),
            coefficients.as_ref(),
            Conj::No,
            residual.values.as_mut(),
            Par::Seq,
            MemStack::new(&mut memory),
        );
        if (0..n).any(|i| !residual.values[(i, 0)].is_finite()) {
            return Err("rank-revealing residual is not finite");
        }
    }
    let mut permuted = if rank == n {
        triangular_solve::solve_upper_triangular_in_place(
            right.as_ref().transpose(),
            transformed.as_mut(),
            Par::Seq,
        );
        transformed
    } else {
        let right_block = qr::no_pivoting::factor::recommended_block_size::<f64>(n, rank);
        let mut right_coefficients = allocate_matrix(right_block, rank, |_, _| 0.0)?;
        let mut memory = MemBuffer::try_new(qr::no_pivoting::factor::qr_in_place_scratch::<f64>(
            n,
            rank,
            right_block,
            Par::Seq,
            Default::default(),
        ))
        .map_err(|_| "rank-revealing response transpose workspace allocation failed")?;
        qr::no_pivoting::factor::qr_in_place(
            right.as_mut(),
            right_coefficients.as_mut(),
            Par::Seq,
            MemStack::new(&mut memory),
            Default::default(),
        );
        if (0..rank).any(|i| right[(i, i)] == 0.0 || !right[(i, i)].is_finite()) {
            return Err("rank-revealing response transpose factor is singular");
        }
        triangular_solve::solve_lower_triangular_in_place(
            right.as_ref().submatrix(0, 0, rank, rank).transpose(),
            transformed.as_mut().submatrix_mut(0, 0, rank, 1),
            Par::Seq,
        );
        for j in 0..rank {
            for i in 0..=j {
                right[(i, j)] = if i == j { 1.0 } else { 0.0 };
            }
        }
        let mut permuted = allocate_matrix(
            n,
            1,
            |i, _| if i < rank { transformed[(i, 0)] } else { 0.0 },
        )?;
        let mut memory = MemBuffer::try_new(
            hh::apply_block_householder_sequence_on_the_left_in_place_scratch::<f64>(
                n,
                right_block,
                1,
            ),
        )
        .map_err(|_| "rank-revealing response right workspace allocation failed")?;
        hh::apply_block_householder_sequence_on_the_left_in_place_with_conj(
            right.as_ref(),
            right_coefficients.as_ref(),
            Conj::No,
            permuted.as_mut(),
            Par::Seq,
            MemStack::new(&mut memory),
        );
        permuted
    };
    let mut restored = allocate_matrix(n, 1, |_, _| 0.0)?;
    faer::perm::permute_rows(
        restored.as_mut(),
        permuted.as_mut().as_ref(),
        permutation.inverse(),
    );
    for i in 0..n {
        result[i] = restored[(i, 0)];
        if !result[i].is_finite() {
            return Err("rank-revealing response multipliers are not finite");
        }
    }
    Ok(Solution {
        multipliers: result,
        residual,
    })
}

/// Follow an inconsistent PSD face's null residual or a negative-reaction
/// direction only as far as the first blocking contact bounds, retiring ties
/// together at the same feasible point. The owner calls this only for a
/// nonterminal native face; finished faces retain their norm.
pub(super) fn blocking_rows(
    selected: &[usize],
    residual: &OrthogonalResidual,
    unilateral: &[bool],
    active: &[bool],
    candidate: &[f64],
    feasible: &mut Mat<f64>,
) -> Result<Vec<usize>, &'static str> {
    let n = active.len();
    let mut direction = allocate_matrix(n, 1, |_, _| 0.0)?;
    let mut blocker: Option<(usize, f64)> = None;
    for (row, &i) in selected.iter().enumerate() {
        let value = residual.values[(row, 0)];
        // An uncertain sign cannot define a null search direction. This affects
        // only intermediate search; the native terminal reaction is unchanged.
        if value.abs() > residual.error_bound {
            direction[(i, 0)] = value;
        }
    }
    for i in 0..n {
        if unilateral[i] && direction[(i, 0)] < 0.0 {
            let ratio = feasible[(i, 0)] / -direction[(i, 0)];
            if blocker.is_none_or(|(_, best)| ratio.total_cmp(&best).is_lt()) {
                blocker = Some((i, ratio));
            }
        }
    }
    if blocker.is_none() {
        for i in 0..n {
            direction[(i, 0)] = candidate[i] - feasible[(i, 0)];
        }
        for i in 0..n {
            if active[i] && unilateral[i] && candidate[i] < 0.0 {
                let ratio = feasible[(i, 0)] / -direction[(i, 0)];
                if blocker.is_none_or(|(_, best)| ratio.total_cmp(&best).is_lt()) {
                    blocker = Some((i, ratio));
                }
            }
        }
    }
    let mut blocked = Vec::new();
    blocked
        .try_reserve_exact(n)
        .map_err(|_| "coupled blocking contact allocation failed")?;
    if let Some((_, step)) = blocker {
        if !step.is_finite() || step < 0.0 {
            return Err("coupled feasible response step is not finite and nonnegative");
        }
        let roundoff = 3.0 * f64::EPSILON / (1.0 - 3.0 * f64::EPSILON);
        for j in 0..n {
            let binding = active[j]
                && unilateral[j]
                && direction[(j, 0)] < 0.0
                && feasible[(j, 0)] / -direction[(j, 0)] == step;
            let change = step * direction[(j, 0)];
            let bound = roundoff * (feasible[(j, 0)].abs() + change.abs());
            let value = feasible[(j, 0)] + change;
            if !value.is_finite() || (unilateral[j] && value < -bound) {
                return Err("coupled response left the feasible multiplier bounds");
            }
            if binding {
                if value.abs() > bound {
                    return Err("coupled blocking contact exceeded its arithmetic bound");
                }
                feasible[(j, 0)] = 0.0;
                blocked.push(j);
            } else {
                feasible[(j, 0)] = if unilateral[j] { value.max(0.0) } else { value };
            }
        }
        if blocked.is_empty() {
            return Err("coupled blocking step reached no contact");
        }
    } else {
        for i in 0..n {
            feasible[(i, 0)] = candidate[i];
        }
    }
    Ok(blocked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simultaneous_contact_bounds_share_one_feasible_pivot() {
        let residual = OrthogonalResidual {
            values: Mat::from_fn(3, 1, |i, _| [-1.0, -2.0, 1.0][i]),
            error_bound: 0.0,
        };
        let mut feasible = Mat::zeros(3, 1);
        let blocked = blocking_rows(
            &[0, 1, 2],
            &residual,
            &[true; 3],
            &[true; 3],
            &[0.0; 3],
            &mut feasible,
        )
        .unwrap();
        assert_eq!(blocked, [0, 1]);
        assert!((0..3).all(|i| feasible[(i, 0)] == 0.0));
    }

    #[test]
    fn uncertain_null_components_preserve_feasible_bounds() {
        let residual = OrthogonalResidual {
            values: Mat::from_fn(2, 1, |i, _| [-1e-10, -1e-4][i]),
            error_bound: 1e-7,
        };
        let mut feasible = Mat::from_fn(2, 1, |i, _| i as f64);
        let blocked = blocking_rows(
            &[0, 1],
            &residual,
            &[true; 2],
            &[true; 2],
            &[1.0; 2],
            &mut feasible,
        )
        .unwrap();
        assert_eq!(blocked, [1]);
        assert_eq!(feasible[(0, 0)], 0.0);
        assert!(feasible[(1, 0)].abs() < 1e-15);
    }

    #[test]
    fn every_negative_target_direction_limits_the_feasible_step() {
        let residual = OrthogonalResidual {
            values: Mat::zeros(2, 1),
            error_bound: 0.0,
        };
        let mut feasible = Mat::from_fn(2, 1, |i, _| if i == 0 { 1.0 } else { 0.0 });
        let blocked = blocking_rows(
            &[0, 1],
            &residual,
            &[true; 2],
            &[true; 2],
            &[-1.0, -1e-20],
            &mut feasible,
        )
        .unwrap();
        assert_eq!(blocked, [1]);
        assert_eq!(feasible[(0, 0)], 1.0);
        assert_eq!(feasible[(1, 0)], 0.0);
    }

    #[test]
    fn factor_residual_stays_orthogonal_under_reaction_cancellation() {
        let epsilon = 1.0 / 1024.0;
        let e2 = epsilon * epsilon;
        let a = [
            [1.0, 1.0, 1.0],
            [1.0, 1.0 + e2, 1.0 - e2],
            [1.0, 1.0 - e2, 1.0 + e2],
        ];
        let delta = 1e-9;
        let rhs = [-2.0 * delta, 1e-4 + delta, -1e-4 + delta];
        let solved = minimum_norm(Mat::from_fn(3, 3, |i, j| a[i][j]), &rhs, 2).unwrap();
        assert!(solved.residual.values.norm_l2() > 1e-10);
        for j in 0..3 {
            let normal = (0..3)
                .map(|i| a[i][j] * solved.residual.values[(i, 0)])
                .sum::<f64>();
            assert!(normal.abs() < 1e-21, "{normal}");
        }
    }

    #[test]
    fn orthogonal_response_retains_minimum_norm_and_rejects_invalid_inputs() {
        for (coefficients, rhs, rank, expected) in [
            ([[1.0, 3.0], [2.0, 6.0]], [4.0, 8.0], 1, [0.4, 1.2]),
            ([[1.0, 3.0], [2.0, 6.0]], [1.0, 0.0], 1, [0.02, 0.06]),
            ([[2.0, 1.0], [0.0, 3.0]], [4.0, 6.0], 2, [1.0, 2.0]),
        ] {
            let solved =
                minimum_norm(Mat::from_fn(2, 2, |i, j| coefficients[i][j]), &rhs, rank).unwrap();
            for i in 0..2 {
                assert!((solved.multipliers[i] - expected[i]).abs() < 1e-12);
            }
        }
        assert!(
            minimum_norm(Mat::from_fn(1, 1, |_, _| f64::NAN), &[1.0], 1)
                .unwrap_err()
                .contains("not finite")
        );
        assert!(
            minimum_norm(Mat::zeros(1, 1), &[f64::INFINITY], 1)
                .unwrap_err()
                .contains("not finite")
        );
        assert!(
            minimum_norm(Mat::zeros(1, 1), &[1.0], 1)
                .unwrap_err()
                .contains("no correcting rank")
        );
        assert!(
            minimum_norm(Mat::zeros(2, 1), &[1.0, 2.0], 1)
                .unwrap_err()
                .contains("dimensions")
        );
    }
}
