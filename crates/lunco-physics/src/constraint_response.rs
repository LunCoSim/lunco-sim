//! Coupled f64 response of prepared geometric equality and unilateral rows.

use avian3d::dynamics::solver::{
    solver_body::{SolverBody, SolverBodyInertia},
    xpbd::{AngularConstraint, PositionConstraint},
};
use bevy::{
    math::DVec3,
    platform::collections::{HashMap, HashSet},
    prelude::Entity,
};
use faer::{
    Mat, MatMut, MatRef, Par,
    dyn_stack::{MemBuffer, MemStack, StackReq},
    matrix_free::{BiLinOp, IdentityPrecond, InitialGuessStatus, LinOp, lsmr},
};

fn dominance_mask(inertias: [&SolverBodyInertia; 2]) -> [bool; 2] {
    match inertias[0].dominance().cmp(&inertias[1].dominance()) {
        std::cmp::Ordering::Greater => [false, true],
        std::cmp::Ordering::Less => [true, false],
        _ => [true, true],
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Gradient {
    pub linear: DVec3,
    pub angular: DVec3,
}

impl Gradient {
    fn dot(self, other: Self) -> f64 {
        self.linear.dot(other.linear) + self.angular.dot(other.angular)
    }

    fn is_finite(self) -> bool {
        self.linear.is_finite() && self.angular.is_finite()
    }

    fn responds(self) -> bool {
        self.linear != DVec3::ZERO || self.angular != DVec3::ZERO
    }
}

/// A scalar violation decreases under `-M^-1 J^T lambda`. Equality multipliers
/// are signed; unilateral multipliers must be nonnegative. Response columns
/// include the native per-constraint dominance mask.
#[derive(Debug)]
pub(crate) struct Row {
    bodies: [Entity; 2],
    gradients: [Gradient; 2],
    applied_gradients: [Gradient; 2],
    responses: [Gradient; 2],
    error: f64,
    tolerance: f64,
    unilateral: bool,
}

impl Row {
    pub(crate) fn new(
        bodies: [Entity; 2],
        gradients: [Gradient; 2],
        inertias: [&SolverBodyInertia; 2],
        error: f64,
        tolerance: f64,
        unilateral: bool,
    ) -> Result<Self, &'static str> {
        if bodies[0] == bodies[1]
            || !error.is_finite()
            || !tolerance.is_finite()
            || tolerance <= 0.0
            || !gradients.into_iter().all(Gradient::is_finite)
        {
            return Err("coupled constraint has invalid geometry, endpoints or tolerance");
        }
        let mask = dominance_mask(inertias);
        let applied_gradients = std::array::from_fn(|i| {
            if mask[i] {
                gradients[i]
            } else {
                Gradient::default()
            }
        });
        let responses = std::array::from_fn(|i| Gradient {
            linear: inertias[i].effective_inv_mass() * applied_gradients[i].linear,
            angular: inertias[i].effective_inv_angular_inertia() * applied_gradients[i].angular,
        });
        if !responses.into_iter().all(Gradient::is_finite) {
            return Err("coupled constraint native response is not finite");
        }
        Ok(Self {
            bodies,
            gradients,
            applied_gradients,
            responses,
            error,
            tolerance,
            unilateral,
        })
    }

    fn response_to(&self, other: &Self) -> f64 {
        let mut response = 0.0;
        for i in 0..2 {
            for j in 0..2 {
                if self.bodies[i] == other.bodies[j] {
                    response += self.gradients[i].dot(other.responses[j]);
                }
            }
        }
        response
    }
}

/// Sum generalized impulses before native position/rotation integration. Large
/// internal reactions can cancel; they must not create intermediate rotations.
pub(crate) fn impulses(
    rows: &[Row],
    multipliers: &[f64],
) -> Result<Vec<(Entity, Gradient)>, &'static str> {
    if rows.len() != multipliers.len() {
        return Err("coupled multiplier ownership is stale");
    }
    let mut indices = HashMap::new();
    let mut output: Vec<(Entity, Gradient)> = Vec::new();
    for (row, &lambda) in rows.iter().zip(multipliers) {
        if !lambda.is_finite() {
            return Err("coupled multiplier is not finite");
        }
        for i in 0..2 {
            let index = *indices.entry(row.bodies[i]).or_insert_with(|| {
                let index = output.len();
                output.push((row.bodies[i], Gradient::default()));
                index
            });
            output[index].1.linear -= row.applied_gradients[i].linear * lambda;
            output[index].1.angular -= row.applied_gradients[i].angular * lambda;
        }
    }
    if !output.iter().all(|(_, impulse)| impulse.is_finite()) {
        return Err("coupled body impulse exceeded its finite numeric range");
    }
    Ok(output)
}

struct CorrectionKernel;
impl PositionConstraint for CorrectionKernel {}
impl AngularConstraint for CorrectionKernel {}

pub(crate) fn apply_body_impulse(
    body: &mut SolverBody,
    inertia: &SolverBodyInertia,
    impulse: Gradient,
) -> Result<(), &'static str> {
    let mut candidate = body.clone();
    let mut immovable = SolverBody::DUMMY;
    if impulse.linear != DVec3::ZERO {
        CorrectionKernel.apply_positional_impulse(
            &mut candidate,
            &mut immovable,
            inertia,
            &SolverBodyInertia::DUMMY,
            impulse.linear,
            DVec3::ZERO,
            DVec3::ZERO,
        );
    }
    if impulse.angular != DVec3::ZERO {
        CorrectionKernel.apply_angular_impulse(
            &mut candidate,
            &mut immovable,
            inertia.effective_inv_angular_inertia(),
            SolverBodyInertia::DUMMY.effective_inv_angular_inertia(),
            impulse.angular,
        );
    }
    if !candidate.delta_position.is_finite()
        || !crate::avian_backend_rotation_is_valid(candidate.delta_rotation.0)
    {
        return Err("coupled body correction is not a finite rigid state");
    }
    *body = candidate;
    Ok(())
}

/// Independent responsive body islands bound solver work. Native topology
/// partitions the rows; arithmetic and pivot choices retain admitted row order.
pub(crate) fn solve(rows: &[Row], max_iterations: usize) -> Result<Vec<f64>, &'static str> {
    let _span = bevy::log::debug_span!("coupled_constraint_response").entered();
    if max_iterations == 0 {
        return Err("coupled constraint iteration limit is zero");
    }
    if rows.iter().all(|row| {
        if row.unilateral {
            row.error <= 0.0
        } else {
            row.error == 0.0
        }
    }) {
        return Ok(vec![0.0; rows.len()]);
    }
    bevy::log::trace!(
        target: "lunco_physics::constraint_response::systems",
        ?rows,
        "admitted coupled response system"
    );
    let active_bodies = rows
        .iter()
        .flat_map(|row| (0..2).filter_map(|i| row.responses[i].responds().then_some(row.bodies[i])))
        .collect::<HashSet<_>>();
    let islands = crate::assembly::dynamic_joint_islands(
        active_bodies.iter().copied(),
        rows.iter().map(|r| (r.bodies[0], r.bodies[1])),
    );
    let mut body_island = HashMap::new();
    for (index, island) in islands.iter().enumerate() {
        for body in island {
            body_island.insert(*body, index);
        }
    }
    let mut groups = vec![Vec::new(); islands.len()];
    for (index, row) in rows.iter().enumerate() {
        if let Some(island) = row.bodies.iter().find_map(|body| body_island.get(body)) {
            groups[*island].push(index);
        } else if (if row.unilateral {
            row.error
        } else {
            row.error.abs()
        }) > row.tolerance
        {
            return Err("coupled constraint has no movable native response");
        }
    }
    groups.sort_by_key(|group| group.first().copied().unwrap_or(usize::MAX));
    let mut output = vec![0.0; rows.len()];
    for group in groups.into_iter().filter(|group| !group.is_empty()) {
        let multipliers = solve_island(rows, &group, max_iterations)?;
        for (index, multiplier) in group.into_iter().zip(multipliers) {
            output[index] = multiplier;
        }
    }
    Ok(output)
}

#[derive(Debug)]
struct PreparedResponse<'a> {
    rows: Vec<&'a Row>,
    bodies: Vec<[usize; 2]>,
    body_count: usize,
    scratch: StackReq,
}

impl<'a> PreparedResponse<'a> {
    fn new(rows: &'a [Row], indices: &[usize]) -> Result<Self, &'static str> {
        let rows = indices.iter().map(|&i| &rows[i]).collect::<Vec<_>>();
        let mut body_map = HashMap::new();
        let mut body_count = 0usize;
        let bodies = rows
            .iter()
            .map(|row| {
                row.bodies.map(|entity| {
                    *body_map.entry(entity).or_insert_with(|| {
                        let index = body_count;
                        body_count += 1;
                        index
                    })
                })
            })
            .collect();
        body_count
            .checked_mul(std::mem::size_of::<Gradient>())
            .filter(|&size| size <= isize::MAX as usize)
            .ok_or("coupled body workspace size overflow")?;
        Ok(Self {
            rows,
            bodies,
            body_count,
            scratch: StackReq::new::<Gradient>(body_count),
        })
    }

    fn operator<'b>(&'b self, selected: &'b [usize], scales: &'b [f64]) -> ResponseOperator<'b> {
        ResponseOperator {
            rows: &self.rows,
            bodies: &self.bodies,
            body_count: self.body_count,
            scratch: self.scratch,
            selected,
            scales,
        }
    }
}

/// Two sparse body reductions apply J M^-1 J^T and its transpose without a
/// dense row matrix. All accumulation follows the admitted scalar-row order.
#[derive(Debug)]
struct ResponseOperator<'a> {
    rows: &'a [&'a Row],
    bodies: &'a [[usize; 2]],
    body_count: usize,
    scratch: StackReq,
    selected: &'a [usize],
    scales: &'a [f64],
}

impl ResponseOperator<'_> {
    fn product(
        &self,
        mut out: MatMut<'_, f64>,
        rhs: MatRef<'_, f64>,
        transpose: bool,
        stack: &mut MemStack,
    ) {
        let (mut work, _) = stack.make_with::<Gradient>(self.body_count, |_| Gradient::default());
        for col in 0..rhs.ncols() {
            work.fill(Gradient::default());
            for (j, &index) in self.selected.iter().enumerate() {
                let row = self.rows[index];
                let x = rhs[(j, col)] * self.scales[j];
                let gradients = if transpose {
                    row.gradients
                } else {
                    row.responses
                };
                for end in 0..2 {
                    let body = &mut work[self.bodies[index][end]];
                    body.linear += gradients[end].linear * x;
                    body.angular += gradients[end].angular * x;
                }
            }
            for (i, &index) in self.selected.iter().enumerate() {
                let row = self.rows[index];
                let gradients = if transpose {
                    row.responses
                } else {
                    row.gradients
                };
                let mut value = 0.0;
                for end in 0..2 {
                    value += gradients[end].dot(work[self.bodies[index][end]]);
                }
                out[(i, col)] = value * self.scales[i];
            }
        }
    }
}

impl LinOp<f64> for ResponseOperator<'_> {
    fn apply_scratch(&self, _rhs_ncols: usize, _par: Par) -> StackReq {
        self.scratch
    }
    fn nrows(&self) -> usize {
        self.selected.len()
    }
    fn ncols(&self) -> usize {
        self.selected.len()
    }
    fn apply(&self, out: MatMut<'_, f64>, rhs: MatRef<'_, f64>, _par: Par, stack: &mut MemStack) {
        self.product(out, rhs, false, stack);
    }
    fn conj_apply(
        &self,
        out: MatMut<'_, f64>,
        rhs: MatRef<'_, f64>,
        par: Par,
        stack: &mut MemStack,
    ) {
        self.apply(out, rhs, par, stack);
    }
}

impl BiLinOp<f64> for ResponseOperator<'_> {
    fn transpose_apply_scratch(&self, _rhs_ncols: usize, _par: Par) -> StackReq {
        self.scratch
    }
    fn transpose_apply(
        &self,
        out: MatMut<'_, f64>,
        rhs: MatRef<'_, f64>,
        _par: Par,
        stack: &mut MemStack,
    ) {
        self.product(out, rhs, true, stack);
    }
    fn adjoint_apply(
        &self,
        out: MatMut<'_, f64>,
        rhs: MatRef<'_, f64>,
        par: Par,
        stack: &mut MemStack,
    ) {
        self.transpose_apply(out, rhs, par, stack);
    }
}

fn solve_island(
    rows: &[Row],
    indices: &[usize],
    max_iterations: usize,
) -> Result<Vec<f64>, &'static str> {
    let _span = bevy::log::debug_span!("coupled_island_response").entered();
    let n = indices.len();
    let prepared = PreparedResponse::new(rows, indices)?;
    let all = (0..n).collect::<Vec<_>>();
    let unscaled = vec![1.0; n];
    let full = prepared.operator(&all, &unscaled);
    let diagonal_scales = indices
        .iter()
        .map(|&i| {
            let diagonal = rows[i].response_to(&rows[i]).abs();
            if diagonal == 0.0 {
                1.0
            } else {
                1.0 / diagonal.sqrt()
            }
        })
        .collect::<Vec<_>>();
    if diagonal_scales.iter().any(|x| !x.is_finite()) {
        return Err("coupled constraint diagonal response is not finite");
    }
    let mut full_input = allocate_matrix(n, 1, |_, _| 0.0)?;
    let mut full_output = allocate_matrix(n, 1, |_, _| 0.0)?;
    let mut full_memory = MemBuffer::try_new(prepared.scratch)
        .map_err(|_| "coupled response workspace allocation failed")?;
    // Admission and later re-admission use the same residual bound. Excluded
    // inequalities remain in the full response check after every correction.
    let mut active = indices
        .iter()
        .map(|&i| !rows[i].unilateral || rows[i].error > rows[i].tolerance)
        .collect::<Vec<_>>();
    for _ in 0..max_iterations {
        let _span = bevy::log::debug_span!("coupled_active_set").entered();
        let selected = active
            .iter()
            .enumerate()
            .filter_map(|(i, &yes)| yes.then_some(i))
            .collect::<Vec<_>>();
        let scales = selected
            .iter()
            .map(|&i| diagonal_scales[i])
            .collect::<Vec<_>>();
        let rhs = selected
            .iter()
            .enumerate()
            .map(|(i, &row)| rows[indices[row]].error * scales[i])
            .collect::<Vec<_>>();
        let operator = prepared.operator(&selected, &scales);
        let solved = if selected.is_empty() {
            Vec::new()
        } else {
            // Reserve three orders below the tightest scaled row tolerance.
            // The nonlinear owner validates native geometry before writeback.
            let absolute_bound = selected
                .iter()
                .enumerate()
                .map(|(i, &row)| rows[indices[row]].tolerance * scales[i])
                .fold(f64::INFINITY, f64::min)
                * 1.0e-3;
            if !absolute_bound.is_finite() || absolute_bound <= 0.0 {
                return Err("coupled scaled row tolerance is not finite and positive");
            }
            solve_minimum_residual(&operator, &rhs, absolute_bound, max_iterations)?
        };
        let mut multipliers = vec![0.0; n];
        for (i, &row) in selected.iter().enumerate() {
            multipliers[row] = solved[i] * scales[i];
        }
        if multipliers.iter().any(|x| !x.is_finite()) {
            return Err("coupled constraint multiplier exceeded its finite numeric range");
        }
        // Stable principal pivots release pulling unilateral reactions before
        // admitting an inactive inequality disturbed by the coupled solution.
        if let Some(i) =
            (0..n).find(|&i| active[i] && rows[indices[i]].unilateral && multipliers[i] < 0.0)
        {
            active[i] = false;
            continue;
        }
        for i in 0..n {
            full_input[(i, 0)] = multipliers[i];
        }
        full.apply(
            full_output.as_mut(),
            full_input.as_ref(),
            Par::Seq,
            MemStack::new(&mut full_memory),
        );
        let residual = (0..n)
            .map(|i| rows[indices[i]].error - full_output[(i, 0)])
            .collect::<Vec<_>>();
        if residual.iter().any(|x| !x.is_finite()) {
            return Err("coupled constraint residual exceeded its finite numeric range");
        }
        // Redundant inequalities may have different slack. An oversatisfied
        // active row is released even when the least-squares multiplier is
        // positive; its boundary need not remain an equality.
        if let Some(i) = (0..n).find(|&i| {
            active[i] && rows[indices[i]].unilateral && residual[i] < -rows[indices[i]].tolerance
        }) {
            active[i] = false;
            continue;
        }
        if let Some(i) = (0..n).find(|&i| !active[i] && residual[i] > rows[indices[i]].tolerance) {
            active[i] = true;
            continue;
        }
        // Redundant nonlinear rows can have an inconsistent linearized RHS.
        // Their least-squares correction is revalidated by the geometric owner.
        return Ok(multipliers);
    }
    Err("coupled unilateral constraint pivots did not converge")
}

fn allocate_matrix(
    nrows: usize,
    ncols: usize,
    value: impl FnMut(usize, usize) -> f64,
) -> Result<Mat<f64>, &'static str> {
    let mut matrix = Mat::<f64>::new();
    matrix
        .try_reserve(nrows, ncols)
        .map_err(|_| "coupled constraint matrix allocation failed")?;
    matrix.resize_with(nrows, ncols, value);
    Ok(matrix)
}

fn solve_minimum_residual(
    operator: &ResponseOperator,
    rhs: &[f64],
    absolute_bound: f64,
    max_iterations: usize,
) -> Result<Vec<f64>, &'static str> {
    let _span = bevy::log::debug_span!("coupled_response_krylov").entered();
    let n = rhs.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    if rhs.iter().any(|x| !x.is_finite()) {
        return Err("scaled coupled constraint system is not finite");
    }
    let b = allocate_matrix(n, 1, |i, _| rhs[i])?;
    let mut out = allocate_matrix(n, 1, |_, _| 0.0)?;
    let preconditioner = IdentityPrecond { dim: n };
    // LSMR retains an n-by-RHS search-history column during sparse products.
    // Its live storage must coexist with the published solver workspace.
    let scratch = StackReq::all_of(&[
        lsmr::lsmr_scratch::<f64>(preconditioner, operator, 1, Par::Seq),
        faer::linalg::temp_mat_scratch::<f64>(n, 1),
    ]);
    let mut memory =
        MemBuffer::try_new(scratch).map_err(|_| "coupled Krylov workspace allocation failed")?;
    let mut params = lsmr::LsmrParams::<f64>::default();
    params.initial_guess = InitialGuessStatus::Zero;
    params.max_iters = max_iterations;
    // Resolve the normal residual to f64 precision without demanding a
    // relative reduction below the absolute arithmetic floor for a small RHS.
    // Faer's LSMR stopping test consumes rel_tolerance; translate the complete
    // owner bound through ||A^T b|| using the same native sparse adjoint.
    let mut normal_rhs = allocate_matrix(n, 1, |_, _| 0.0)?;
    operator.adjoint_apply(
        normal_rhs.as_mut(),
        b.as_ref(),
        Par::Seq,
        MemStack::new(&mut memory),
    );
    let normal_norm = normal_rhs.norm_l2();
    if !normal_norm.is_finite() {
        return Err("coupled normal right-hand side is not finite");
    }
    let relative_bound: f64 = 1.0e-12;
    params.rel_tolerance = if normal_norm > 0.0 {
        relative_bound.max(absolute_bound / normal_norm)
    } else {
        relative_bound
    };
    let info = lsmr::lsmr(
        out.as_mut(),
        preconditioner,
        operator,
        b.as_ref(),
        params,
        |_| {},
        Par::Seq,
        MemStack::new(&mut memory),
    )
    .map_err(|error| {
        bevy::log::warn!(
            rows = n,
            max_iterations,
            ?error,
            "coupled Krylov convergence failed"
        );
        "coupled minimum-residual solve did not converge"
    })?;
    bevy::log::trace!(
        target: "lunco_physics::constraint_response::iterations",
        rows = n,
        iterations = info.iter_count,
        absolute_residual = info.abs_residual,
        relative_residual = info.rel_residual,
        "completed coupled Krylov solve"
    );
    Ok((0..n).map(|i| out[(i, 0)]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::{math::SymmetricMatrix, prelude::LockedAxes};

    fn inertia(inverse_mass: f64) -> SolverBodyInertia {
        SolverBodyInertia::new(
            inverse_mass,
            SymmetricMatrix::ZERO,
            LockedAxes::new(),
            0,
            true,
        )
    }
    fn linear(x: f64) -> Gradient {
        Gradient {
            linear: DVec3::X * x,
            ..Default::default()
        }
    }

    #[test]
    fn sparse_native_response_and_adjoint_match_dominance_weighted_entries() {
        let a = Entity::from_bits(1);
        let b = Entity::from_bits(2);
        let wall = Entity::from_bits(3);
        let high =
            SolverBodyInertia::new(0.25, SymmetricMatrix::IDENTITY, LockedAxes::new(), 2, true);
        let low = inertia(1.0);
        let fixed = SolverBodyInertia::DUMMY;
        let rows = [
            Row::new(
                [a, b],
                [
                    Gradient {
                        linear: -DVec3::X,
                        angular: DVec3::Y,
                    },
                    Gradient {
                        linear: DVec3::X,
                        angular: DVec3::Z,
                    },
                ],
                [&high, &low],
                1.0,
                1e-9,
                false,
            )
            .unwrap(),
            Row::new(
                [b, wall],
                [
                    Gradient {
                        linear: DVec3::Y,
                        angular: DVec3::X,
                    },
                    Gradient {
                        linear: -DVec3::Y,
                        angular: -DVec3::X,
                    },
                ],
                [&low, &fixed],
                1.0,
                1e-9,
                true,
            )
            .unwrap(),
            Row::new(
                [a, wall],
                [
                    Gradient {
                        linear: DVec3::X + DVec3::Y,
                        angular: DVec3::Z,
                    },
                    Gradient {
                        linear: -DVec3::X - DVec3::Y,
                        angular: -DVec3::Z,
                    },
                ],
                [&high, &fixed],
                1.0,
                1e-9,
                false,
            )
            .unwrap(),
        ];
        let indices = [0, 1, 2];
        let scales = [0.75, 1.25, 0.5];
        let prepared = PreparedResponse::new(&rows, &indices).unwrap();
        let op = prepared.operator(&indices, &scales);
        let rhs = allocate_matrix(3, 2, |i, j| (i as f64 - 1.0) * (j as f64 + 2.0) + 0.25).unwrap();
        let mut out = allocate_matrix(3, 2, |_, _| 0.0).unwrap();
        let mut memory = MemBuffer::try_new(prepared.scratch).unwrap();
        for transpose in [false, true] {
            op.product(
                out.as_mut(),
                rhs.as_ref(),
                transpose,
                MemStack::new(&mut memory),
            );
            for i in 0..3 {
                for col in 0..2 {
                    let expected = (0..3)
                        .map(|j| {
                            let entry = if transpose {
                                rows[j].response_to(&rows[i])
                            } else {
                                rows[i].response_to(&rows[j])
                            };
                            entry * scales[i] * scales[j] * rhs[(j, col)]
                        })
                        .sum::<f64>();
                    assert!((out[(i, col)] - expected).abs() < 1e-12);
                }
            }
        }
        assert_ne!(rows[0].response_to(&rows[2]), rows[2].response_to(&rows[0]));
    }

    #[test]
    fn coupled_response_closes_heavy_attachment_and_releases_unilateral_rows() {
        let a = Entity::from_bits(1);
        let b = Entity::from_bits(2);
        let wall = Entity::from_bits(3);
        let heavy = inertia(0.001);
        let light = inertia(1.0);
        let fixed = SolverBodyInertia::DUMMY;
        let rows = [
            Row::new(
                [a, b],
                [linear(-1.0), linear(1.0)],
                [&heavy, &light],
                0.0,
                1e-9,
                false,
            )
            .unwrap(),
            Row::new(
                [b, wall],
                [linear(1.0), linear(-1.0)],
                [&light, &fixed],
                2.1,
                1e-9,
                true,
            )
            .unwrap(),
            Row::new(
                [b, wall],
                [linear(-1.0), linear(1.0)],
                [&light, &fixed],
                -3.0,
                1e-9,
                true,
            )
            .unwrap(),
            Row::new(
                [b, wall],
                [linear(1.0), linear(-1.0)],
                [&light, &fixed],
                1.0,
                1e-9,
                true,
            )
            .unwrap(),
        ];
        let result = solve(&rows, 64).unwrap();
        assert!((result[0] + 2100.0).abs() < 1e-8);
        assert!((result[1] - 2102.1).abs() < 1e-8);
        assert_eq!(result[2], 0.0);
        assert_eq!(result[3], 0.0);
        assert!((result[0] * 0.001 + 2.1).abs() < 1e-10);
        assert!((result[0] + result[1] - 2.1).abs() < 1e-10);
    }

    #[test]
    fn normal_stopping_budget_preserves_small_scale_heavy_attachment_response() {
        let a = Entity::from_bits(1);
        let b = Entity::from_bits(2);
        let wall = Entity::from_bits(3);
        let heavy = inertia(0.001);
        let light = inertia(1.0);
        let fixed = SolverBodyInertia::DUMMY;
        for scale in [1.0, 1e-6, 1e-9] {
            let rows = [
                Row::new(
                    [a, b],
                    [linear(-1.0), linear(1.0)],
                    [&heavy, &light],
                    0.0,
                    1e-9 * scale,
                    false,
                )
                .unwrap(),
                Row::new(
                    [b, wall],
                    [linear(1.0), linear(-1.0)],
                    [&light, &fixed],
                    2.1 * scale,
                    1e-9 * scale,
                    true,
                )
                .unwrap(),
            ];
            let result = solve(&rows, 64).unwrap();
            assert!((result[0] / scale + 2100.0).abs() < 1e-8);
            assert!((result[1] / scale - 2102.1).abs() < 1e-8);
            assert!((result[0] + result[1] - 2.1 * scale).abs() < 1e-10 * scale);
        }
    }

    #[test]
    fn sparse_krylov_workspace_covers_many_rows_over_few_bodies() {
        let body = inertia(1.0);
        for count in 18..200 {
            let rows = (0..count)
                .map(|i| {
                    let branch = i % 17;
                    Row::new(
                        [Entity::from_bits(1), Entity::from_bits(branch + 2)],
                        [linear(-1.0), linear(1.0)],
                        [&body, &body],
                        1.0 + branch as f64 * 0.03,
                        1e-9,
                        false,
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>();
            let result = solve(&rows, 128).unwrap();
            for row in &rows {
                let response = rows
                    .iter()
                    .zip(&result)
                    .map(|(other, &impulse)| row.response_to(other) * impulse)
                    .sum::<f64>();
                assert!((response - row.error).abs() <= row.tolerance);
            }
        }
    }

    #[test]
    fn unilateral_admission_and_readmission_share_the_residual_tolerance() {
        let a = Entity::from_bits(1);
        let b = Entity::from_bits(2);
        let wall = Entity::from_bits(3);
        let moving = inertia(1.0);
        let fixed = SolverBodyInertia::DUMMY;
        let tolerance = 1e-6;
        for error in [-0.5 * tolerance, 0.0, 0.5 * tolerance, tolerance] {
            let row = Row::new(
                [b, wall],
                [linear(-1.0), linear(1.0)],
                [&moving, &fixed],
                error,
                tolerance,
                true,
            )
            .unwrap();
            assert_eq!(solve(&[row], 64).unwrap(), [0.0]);
        }
        // A satisfied contact must still enter when another constraint's
        // correction would move its residual beyond the same bound.
        for error in [-0.5 * tolerance, 0.5 * tolerance, 2.0 * tolerance] {
            let rows = [
                Row::new(
                    [a, b],
                    [linear(-1.0), linear(1.0)],
                    [&moving, &moving],
                    4.0 * tolerance,
                    tolerance,
                    false,
                )
                .unwrap(),
                Row::new(
                    [b, wall],
                    [linear(-1.0), linear(1.0)],
                    [&moving, &fixed],
                    error,
                    tolerance,
                    true,
                )
                .unwrap(),
            ];
            let result = solve(&rows, 64).unwrap();
            assert!(result[1] > 0.0);
            assert!((result[0] - (4.0 * tolerance + error)).abs() < 1e-12);
            assert!((result[1] - (4.0 * tolerance + 2.0 * error)).abs() < 1e-12);
        }
    }

    #[test]
    fn rank_deficient_response_retains_minimum_norm_contact_multiplier() {
        let a = Entity::from_bits(1);
        let wall = Entity::from_bits(2);
        let moving = inertia(1.0);
        let fixed = SolverBodyInertia::DUMMY;
        let row = |unilateral| {
            Row::new(
                [a, wall],
                [linear(1.0), linear(-1.0)],
                [&moving, &fixed],
                1.0,
                1e-9,
                unilateral,
            )
            .unwrap()
        };
        let result = solve(&[row(false), row(false), row(true)], 64).unwrap();
        assert!((result.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        // Equal scaled columns have one minimum-norm reaction: all three
        // multipliers are 1/3. The contact multiplier also feeds native friction.
        assert!(
            (result[2] - 1.0 / 3.0).abs() < 1e-10,
            "minimum-norm contact reaction changed: {result:?}"
        );
    }

    #[test]
    fn coupled_response_retains_redundancy_and_rejects_incompatible_locked_rows() {
        let a = Entity::from_bits(1);
        let b = Entity::from_bits(2);
        let moving = inertia(1.0);
        let fixed = SolverBodyInertia::DUMMY;
        let row = || {
            Row::new(
                [a, b],
                [linear(1.0), linear(-1.0)],
                [&moving, &fixed],
                2.0,
                1e-9,
                false,
            )
            .unwrap()
        };
        let result = solve(&[row(), row()], 64).unwrap();
        assert!((result[0] + result[1] - 2.0).abs() < 1e-12);
        let invalid = Row::new(
            [a, b],
            [linear(1.0), linear(-1.0)],
            [&fixed, &fixed],
            1.0,
            1e-9,
            false,
        )
        .unwrap();
        assert!(solve(&[invalid], 64).is_err());
        assert!(
            Row::new(
                [a, b],
                [linear(f64::NAN), linear(1.0)],
                [&moving, &fixed],
                0.0,
                1e-9,
                true
            )
            .is_err()
        );
        assert!(solve(&[row()], 0).is_err());
    }
}
