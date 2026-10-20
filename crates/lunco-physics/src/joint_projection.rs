//! Hard geometric closure composed from public joint frames and impulse kernels.

use crate::constraint_response::{Gradient, Row};
use avian3d::dynamics::joints::EntityConstraint;
use avian3d::dynamics::solver::{
    solver_body::{SolverBody, SolverBodyInertia},
    xpbd::joints::PointConstraintShared,
};
use avian3d::prelude::*;
use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
};

#[derive(Clone, Copy, Debug)]
enum GeometryRow {
    Anchors,
    Orientation,
    AlignedAxes(DVec3),
    Angle(DVec3, AngleLimit),
    Swing(DVec3, AngleLimit),
    Twist(DVec3, AngleLimit),
    SliderAlignment(DVec3),
    SliderLimit(DVec3, DistanceLimit),
    Distance(DistanceLimit),
}

pub(crate) struct JointGeometry {
    anchors: [DVec3; 2],
    bases: Option<[DQuat; 2]>,
    rows: [Option<GeometryRow>; 3],
}

pub(crate) trait ProjectionJoint: Component + EntityConstraint<2> {
    fn geometry(&self) -> Result<JointGeometry, &'static str>;
}

#[derive(Clone, Copy)]
pub(crate) enum JointDrive {
    Linear(LinearMotor, DVec3),
    Angular(AngularMotor, DVec3),
}

fn hard(compliance: f64, row: Option<GeometryRow>) -> Result<Option<GeometryRow>, &'static str> {
    if !compliance.is_finite() || compliance < 0.0 {
        return Err("joint compliance must be finite and nonnegative");
    }
    Ok(if compliance == 0.0 { row } else { None })
}

macro_rules! framed_joint {
    ($kind:ty, $joint:ident, $rows:expr) => {
        impl ProjectionJoint for $kind {
            fn geometry(&self) -> Result<JointGeometry, &'static str> {
                let $joint = self;
                Ok(JointGeometry {
                    anchors: [
                        self.local_anchor1()
                            .ok_or("first local joint anchor is unresolved")?,
                        self.local_anchor2()
                            .ok_or("second local joint anchor is unresolved")?,
                    ],
                    bases: Some([
                        self.local_basis1()
                            .ok_or("first local joint basis is unresolved")?,
                        self.local_basis2()
                            .ok_or("second local joint basis is unresolved")?,
                    ]),
                    rows: $rows,
                })
            }
        }
    };
}
framed_joint!(
    FixedJoint,
    j,
    [
        hard(j.angle_compliance, Some(GeometryRow::Orientation))?,
        hard(j.point_compliance, Some(GeometryRow::Anchors))?,
        None
    ]
);
framed_joint!(
    RevoluteJoint,
    j,
    [
        hard(
            j.align_compliance,
            Some(GeometryRow::AlignedAxes(j.hinge_axis))
        )?,
        hard(
            j.limit_compliance,
            j.angle_limit
                .map(|limit| GeometryRow::Angle(j.hinge_axis, limit))
        )?,
        hard(j.point_compliance, Some(GeometryRow::Anchors))?
    ]
);
framed_joint!(
    PrismaticJoint,
    j,
    [
        hard(j.angle_compliance, Some(GeometryRow::Orientation))?,
        hard(
            j.align_compliance,
            Some(GeometryRow::SliderAlignment(j.slider_axis))
        )?,
        hard(
            j.limit_compliance,
            j.limits
                .map(|limit| GeometryRow::SliderLimit(j.slider_axis, limit))
        )?
    ]
);
framed_joint!(
    SphericalJoint,
    j,
    [
        hard(j.point_compliance, Some(GeometryRow::Anchors))?,
        hard(
            j.swing_compliance,
            j.swing_limit
                .map(|limit| GeometryRow::Swing(j.twist_axis, limit))
        )?,
        hard(
            j.twist_compliance,
            j.twist_limit
                .map(|limit| GeometryRow::Twist(j.twist_axis, limit))
        )?
    ]
);
impl ProjectionJoint for DistanceJoint {
    fn geometry(&self) -> Result<JointGeometry, &'static str> {
        Ok(JointGeometry {
            anchors: [
                self.local_anchor1()
                    .ok_or("first local joint anchor is unresolved")?,
                self.local_anchor2()
                    .ok_or("second local joint anchor is unresolved")?,
            ],
            bases: None,
            rows: [
                hard(self.compliance, Some(GeometryRow::Distance(self.limits)))?,
                None,
                None,
            ],
        })
    }
}

/// Shared prepared joint frames, hard channels and completed-step multipliers.
/// Repeated closure consumes only the geometric rows.
#[derive(Component, Clone, Debug, Default)]
pub(crate) struct JointProjection {
    point: PointConstraintShared,
    bases: Option<[DQuat; 2]>,
    rows: [Option<GeometryRow>; 3],
    prepared: bool,
    pub(crate) position_lagrange: DVec3,
    pub(crate) rotation_lagrange: DVec3,
    pub(crate) motor_impulse: f64,
    pub(crate) damping_linear_impulse: DVec3,
    pub(crate) damping_angular_impulse: DVec3,
    pub(crate) pending_reaction: Option<JointForces>,
}

/// A row's current constrained subspace, materialized only for an impulse solve.
#[derive(Clone, Copy)]
enum ConstraintSpace {
    Full,
    Along(DVec3),
    Perpendicular(DVec3),
}

impl ConstraintSpace {
    fn directions(self) -> [Option<DVec3>; 3] {
        match self {
            Self::Full => [Some(DVec3::X), Some(DVec3::Y), Some(DVec3::Z)],
            Self::Along(axis) => [Some(axis), None, None],
            Self::Perpendicular(axis) => {
                let first = axis.any_orthonormal_vector();
                [Some(first), Some(axis.cross(first).normalize()), None]
            }
        }
    }
}

enum GeometryError {
    Position {
        levers: [DVec3; 2],
        error: DVec3,
        space: ConstraintSpace,
    },
    Angular {
        error: DVec3,
        space: ConstraintSpace,
    },
}

/// One native impulse block assembled from scalar coupled response rows.
pub(crate) struct JointBlock {
    positional: bool,
    columns: Vec<(usize, DVec3)>,
}

impl JointBlock {
    pub(crate) fn record(
        &self,
        projection: &mut JointProjection,
        multipliers: &[f64],
    ) -> Result<(), &'static str> {
        let impulse = self
            .columns
            .iter()
            .fold(DVec3::ZERO, |sum, (index, direction)| {
                sum + *direction * multipliers[*index]
            });
        let position = projection.position_lagrange
            + if self.positional {
                impulse
            } else {
                DVec3::ZERO
            };
        let rotation = projection.rotation_lagrange
            - if !self.positional {
                impulse
            } else {
                DVec3::ZERO
            };
        if !impulse.is_finite() || !position.is_finite() || !rotation.is_finite() {
            return Err("coupled joint reaction is not finite");
        }
        projection.position_lagrange = position;
        projection.rotation_lagrange = rotation;
        Ok(())
    }
}

impl JointProjection {
    pub(crate) fn prepare(
        &mut self,
        geometry: JointGeometry,
        bodies: [&RigidBodyQueryReadOnlyItem; 2],
    ) -> Result<(), &'static str> {
        self.prepared = false;
        if !geometry.anchors.into_iter().all(DVec3::is_finite)
            || !bodies.iter().all(|body| {
                body.position.0.is_finite()
                    && body.center_of_mass.0.is_finite()
                    && crate::avian_backend_rotation_is_valid(body.rotation.0)
            })
            || geometry.bases.is_some_and(|bases| {
                !bases
                    .into_iter()
                    .all(crate::avian_backend_rotation_is_valid)
            })
        {
            return Err("joint projection requires finite rigid frames");
        }
        for row in geometry.rows.into_iter().flatten() {
            match row {
                GeometryRow::AlignedAxes(axis)
                | GeometryRow::Angle(axis, _)
                | GeometryRow::Swing(axis, _)
                | GeometryRow::Twist(axis, _)
                | GeometryRow::SliderAlignment(axis)
                | GeometryRow::SliderLimit(axis, _)
                    if !axis.is_finite() || !axis.is_normalized() =>
                {
                    return Err("joint projection requires a unit joint axis");
                }
                _ => {}
            }
            let limits = match row {
                GeometryRow::Angle(_, limit)
                | GeometryRow::Swing(_, limit)
                | GeometryRow::Twist(_, limit) => Some((limit.min, limit.max)),
                GeometryRow::SliderLimit(_, limit) | GeometryRow::Distance(limit) => {
                    Some((limit.min, limit.max))
                }
                _ => None,
            };
            if limits.is_some_and(|(min, max)| {
                min.is_nan()
                    || max.is_nan()
                    || min > max
                    || min == f64::INFINITY
                    || max == f64::NEG_INFINITY
            }) {
                return Err("joint projection limits are invalid");
            }
        }
        self.point
            .prepare(bodies, geometry.anchors[0], geometry.anchors[1]);
        self.bases = geometry.bases.map(|basis| {
            [
                bodies[0].rotation.0 * basis[0],
                bodies[1].rotation.0 * basis[1],
            ]
        });
        self.rows = geometry.rows;
        self.position_lagrange = DVec3::ZERO;
        self.rotation_lagrange = DVec3::ZERO;
        self.motor_impulse = 0.0;
        self.damping_linear_impulse = DVec3::ZERO;
        self.damping_angular_impulse = DVec3::ZERO;
        self.prepared = true;
        Ok(())
    }

    pub(crate) fn integrate_drive(
        &mut self,
        drive: JointDrive,
        bodies: [&mut SolverBody; 2],
        inertias: [&SolverBodyInertia; 2],
        dt: f64,
    ) -> Result<(), &'static str> {
        use crate::drive::{CoordinateDrive, integrate_coordinate_drive};
        if !self.prepared {
            return Err("joint drive has no admitted frame geometry");
        }
        let bases = self
            .bases
            .ok_or("joint drive requires resolved joint frames")?;
        let frames = [
            bodies[0].delta_rotation.0 * bases[0],
            bodies[1].delta_rotation.0 * bases[1],
        ];
        let (parameters, coordinate, axis, linear, angular, rotational) = match drive {
            JointDrive::Linear(motor, local_axis) => {
                let axis = unit(frames[0] * unit(local_axis)?)?;
                let levers = [
                    bodies[0].delta_rotation * self.point.world_r1,
                    bodies[1].delta_rotation * self.point.world_r2,
                ];
                let separation = bodies[1].delta_position - bodies[0].delta_position + levers[1]
                    - levers[0]
                    + self.point.center_difference;
                (
                    CoordinateDrive {
                        model: motor.motor_model,
                        target_position: motor.target_position,
                        target_velocity: motor.target_velocity,
                        max_force: motor.max_force,
                    },
                    separation.dot(axis),
                    axis,
                    [-axis, axis],
                    [
                        -levers[0].cross(axis) + axis.cross(separation),
                        levers[1].cross(axis),
                    ],
                    false,
                )
            }
            JointDrive::Angular(motor, local_axis) => {
                let (angle, axis) =
                    crate::joint::angular_frame_coordinate(frames[0], frames[1], local_axis)
                        .ok_or("joint drive requires admitted angular frames and axis")?;
                (
                    CoordinateDrive {
                        model: motor.motor_model,
                        target_position: motor.target_position,
                        target_velocity: motor.target_velocity,
                        max_force: motor.max_torque,
                    },
                    angle,
                    axis,
                    [DVec3::ZERO; 2],
                    [-axis, axis],
                    true,
                )
            }
        };
        let impulse = integrate_coordinate_drive(
            bodies, inertias, linear, angular, parameters, coordinate, dt,
        )?;
        let multiplier = axis * (impulse * dt);
        let position = self.position_lagrange - if rotational { DVec3::ZERO } else { multiplier };
        let rotation = self.rotation_lagrange + if rotational { multiplier } else { DVec3::ZERO };
        let motor = self.motor_impulse + impulse;
        if !position.is_finite() || !rotation.is_finite() || !motor.is_finite() {
            return Err("joint drive completed-step reaction is not representable");
        }
        self.position_lagrange = position;
        self.rotation_lagrange = rotation;
        self.motor_impulse = motor;
        Ok(())
    }

    fn row_error(
        &self,
        row: GeometryRow,
        bodies: [&SolverBody; 2],
    ) -> Result<Option<GeometryError>, &'static str> {
        let frames = || {
            let bases = self
                .bases
                .ok_or("angular joint projection requires prepared bases")?;
            Ok::<_, &'static str>([
                bodies[0].delta_rotation.0 * bases[0],
                bodies[1].delta_rotation.0 * bases[1],
            ])
        };
        let error = match row {
            GeometryRow::Anchors
            | GeometryRow::Distance(..)
            | GeometryRow::SliderAlignment(..)
            | GeometryRow::SliderLimit(..) => {
                let levers = [
                    bodies[0].delta_rotation * self.point.world_r1,
                    bodies[1].delta_rotation * self.point.world_r2,
                ];
                let separation = (bodies[1].delta_position - bodies[0].delta_position)
                    + (levers[1] - levers[0])
                    + self.point.center_difference;
                let (error, space) = match row {
                    GeometryRow::Anchors => (separation, ConstraintSpace::Full),
                    GeometryRow::Distance(limit) => {
                        let length = separation.length();
                        if length == 0.0 {
                            if limit.min > 0.0 {
                                return Err(
                                    "separated distance anchors have no resolved direction",
                                );
                            }
                            if limit.max == 0.0 {
                                return Ok(Some(GeometryError::Position {
                                    levers,
                                    error: separation,
                                    space: ConstraintSpace::Full,
                                }));
                            }
                            return Ok(None);
                        }
                        (
                            separation * ((length - length.clamp(limit.min, limit.max)) / length),
                            ConstraintSpace::Along(separation / length),
                        )
                    }
                    GeometryRow::SliderAlignment(axis) => {
                        let axis = unit(frames()?[0] * axis)?;
                        (
                            separation - axis * separation.dot(axis),
                            ConstraintSpace::Perpendicular(axis),
                        )
                    }
                    GeometryRow::SliderLimit(axis, limit) => {
                        let frames = frames()?;
                        let axis = unit(frames[0] * axis)?;
                        (
                            -limit.compute_correction_along_axis(separation, axis),
                            ConstraintSpace::Along(axis),
                        )
                    }
                    _ => unreachable!(),
                };
                GeometryError::Position {
                    levers,
                    error,
                    space,
                }
            }
            _ => {
                let frames = frames()?;
                let mut space = ConstraintSpace::Full;
                let error = match row {
                    GeometryRow::Orientation => {
                        crate::joint::rotation_displacement(frames[0], frames[1])?
                    }
                    GeometryRow::AlignedAxes(axis) => {
                        let first_axis = frames[0] * axis;
                        space = ConstraintSpace::Perpendicular(unit(first_axis)?);
                        axis_error(first_axis, frames[1] * axis, None)?
                    }
                    GeometryRow::Angle(axis, limit) => {
                        let world_axis = unit(frames[0] * axis)?;
                        space = ConstraintSpace::Along(world_axis);
                        let perpendicular = axis.any_orthonormal_vector();
                        angle_error_about(
                            world_axis,
                            frames[0] * perpendicular,
                            frames[1] * perpendicular,
                            limit,
                        )?
                    }
                    GeometryRow::Swing(axis, limit) => {
                        let first = unit(frames[0] * axis)?;
                        let second = unit(frames[1] * axis)?;
                        space = first
                            .cross(second)
                            .try_normalize()
                            .map(ConstraintSpace::Along)
                            .unwrap_or(ConstraintSpace::Perpendicular(first));
                        axis_error(first, second, Some(limit))?
                    }
                    GeometryRow::Twist(axis, limit) => {
                        let perpendicular = axis.any_orthonormal_vector();
                        let normal = unit(frames[0] * axis + frames[1] * axis)?;
                        space = ConstraintSpace::Along(normal);
                        angle_error_about(
                            normal,
                            frames[0] * perpendicular,
                            frames[1] * perpendicular,
                            limit,
                        )?
                    }
                    _ => unreachable!(),
                };
                GeometryError::Angular { error, space }
            }
        };
        Ok(Some(error))
    }

    pub(crate) fn residual(&self, bodies: [&SolverBody; 2]) -> Result<(f64, f64), &'static str> {
        if !self.prepared {
            return Err("joint residual has no admitted geometry");
        }
        let mut residual = (0.0_f64, 0.0_f64);
        for row in self.rows.into_iter().flatten() {
            match self.row_error(row, bodies)? {
                Some(GeometryError::Position { error, .. }) => {
                    residual.0 = residual.0.max(error.length())
                }
                Some(GeometryError::Angular { error, .. }) => {
                    residual.1 = residual.1.max(error.length())
                }
                None => {}
            }
        }
        if !residual.0.is_finite() || !residual.1.is_finite() {
            return Err("joint residual is not finite");
        }
        Ok(residual)
    }

    pub(crate) fn linearize(
        &self,
        entities: [Entity; 2],
        bodies: [&SolverBody; 2],
        inertias: [&SolverBodyInertia; 2],
        tolerances: [f64; 2],
        constraints: &mut Vec<Row>,
    ) -> Result<Vec<JointBlock>, &'static str> {
        if !self.prepared {
            return Err("joint projection has no admitted geometry");
        }
        let mut blocks = Vec::new();
        for row in self.rows.into_iter().flatten() {
            let unilateral = match row {
                GeometryRow::Distance(limit) | GeometryRow::SliderLimit(_, limit) => {
                    limit.min != limit.max
                }
                GeometryRow::Angle(_, limit)
                | GeometryRow::Swing(_, limit)
                | GeometryRow::Twist(_, limit) => limit.min != limit.max,
                _ => false,
            };
            let (levers, error, mut space, tolerance) = match self.row_error(row, bodies)? {
                Some(GeometryError::Position {
                    levers,
                    error,
                    space,
                }) => (Some(levers), error, space, tolerances[0]),
                Some(GeometryError::Angular { error, space }) => {
                    (None, error, space, tolerances[1])
                }
                None => continue,
            };
            if !error.is_finite() {
                return Err("joint projection error is not finite");
            }
            if unilateral {
                let magnitude = error.length();
                if magnitude == 0.0 {
                    continue;
                }
                if !magnitude.is_finite() {
                    return Err("joint limit error is not representable");
                }
                space = ConstraintSpace::Along(error / magnitude);
            }
            let mut columns = Vec::new();
            for direction in space.directions().into_iter().flatten() {
                let gradients = if let Some(r) = levers {
                    [
                        Gradient {
                            linear: -direction,
                            angular: -r[0].cross(direction),
                        },
                        Gradient {
                            linear: direction,
                            angular: r[1].cross(direction),
                        },
                    ]
                } else {
                    [
                        Gradient {
                            linear: DVec3::ZERO,
                            angular: -direction,
                        },
                        Gradient {
                            linear: DVec3::ZERO,
                            angular: direction,
                        },
                    ]
                };
                columns.push((constraints.len(), direction));
                constraints.push(Row::new(
                    entities,
                    gradients,
                    inertias,
                    error.dot(direction),
                    tolerance,
                    unilateral,
                )?);
            }
            blocks.push(JointBlock {
                positional: levers.is_some(),
                columns,
            });
        }
        Ok(blocks)
    }

    #[cfg(test)]
    fn project_once(
        &mut self,
        bodies: [&mut SolverBody; 2],
        inertias: [&SolverBodyInertia; 2],
    ) -> Result<(), &'static str> {
        let [a, b] = bodies;
        let mut rows = Vec::new();
        let blocks = self.linearize(
            [Entity::from_bits(1), Entity::from_bits(2)],
            [&*a, &*b],
            inertias,
            [1e-9; 2],
            &mut rows,
        )?;
        let multipliers = crate::constraint_response::solve(&rows, 1024)?;
        let mut candidates = [a.clone(), b.clone()];
        for (entity, impulse) in crate::constraint_response::impulses(&rows, &multipliers)? {
            let i = if entity == Entity::from_bits(1) { 0 } else { 1 };
            crate::constraint_response::apply_body_impulse(
                &mut candidates[i],
                inertias[i],
                impulse,
            )?;
        }
        for block in blocks {
            block.record(self, &multipliers)?;
        }
        *a = candidates[0].clone();
        *b = candidates[1].clone();
        Ok(())
    }
}

fn unit(vector: DVec3) -> Result<DVec3, &'static str> {
    vector
        .try_normalize()
        .ok_or("joint angular projection has an unresolved axis")
}

fn axis_error(
    first: DVec3,
    second: DVec3,
    limit: Option<AngleLimit>,
) -> Result<DVec3, &'static str> {
    let [first, second] = [unit(first)?, unit(second)?];
    let cross = first.cross(second);
    let sine = cross.length();
    let angle = sine.atan2(first.dot(second));
    let correction = angle - limit.map_or(0.0, |limit| angle.clamp(limit.min, limit.max));
    if correction == 0.0 {
        return Ok(DVec3::ZERO);
    }
    if sine == 0.0 {
        return Err("opposed joint axes have an ambiguous projection branch");
    }
    Ok(cross * (correction / sine))
}

fn angle_error_about(
    axis: DVec3,
    first: DVec3,
    second: DVec3,
    limit: AngleLimit,
) -> Result<DVec3, &'static str> {
    let first = unit(first - axis * first.dot(axis))?;
    let second = unit(second - axis * second.dot(axis))?;
    let angle = axis.dot(first.cross(second)).atan2(first.dot(second));
    Ok(axis * (angle - angle.clamp(limit.min, limit.max)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::math::SymmetricMatrix;

    fn prepared<C: ProjectionJoint>(
        make: impl FnOnce(Entity, Entity) -> C,
        rotations: [DQuat; 2],
    ) -> JointProjection {
        let mut world = World::new();
        let a = world
            .spawn((
                RigidBody::Dynamic,
                Position(DVec3::ZERO),
                Rotation(rotations[0]),
            ))
            .id();
        let b = world
            .spawn((
                RigidBody::Dynamic,
                Position(DVec3::ZERO),
                Rotation(rotations[1]),
            ))
            .id();
        let joint = make(a, b);
        let mut query = world.query::<RigidBodyQueryReadOnly>();
        let [first, second] = query.get_many(&world, [a, b]).unwrap();
        let mut projection = JointProjection::default();
        projection
            .prepare(joint.geometry().unwrap(), [&first, &second])
            .unwrap();
        projection
    }

    fn inertia() -> SolverBodyInertia {
        SolverBodyInertia::new(1.0, SymmetricMatrix::IDENTITY, LockedAxes::new(), 0, true)
    }

    #[test]
    fn redundant_fixed_and_hinge_rows_close_finite_noncommuting_error() {
        let fixed = prepared(FixedJoint::new, [DQuat::IDENTITY; 2]);
        let hinge = prepared(RevoluteJoint::new, [DQuat::IDENTITY; 2]);
        let inertia = inertia();
        let mut a = SolverBody::DUMMY;
        let mut b = SolverBody {
            delta_rotation: Rotation(DQuat::from_euler(EulerRot::XYZ, 0.35, 0.6, 0.45)),
            ..default()
        };
        for _ in 0..32 {
            let first = fixed.residual([&a, &b]).unwrap();
            let second = hinge.residual([&a, &b]).unwrap();
            if first.0.max(first.1).max(second.0).max(second.1) < 1e-9 {
                return;
            }
            let mut rows = Vec::new();
            for joint in [&fixed, &hinge] {
                joint
                    .linearize(
                        [Entity::from_bits(1), Entity::from_bits(2)],
                        [&a, &b],
                        [&inertia; 2],
                        [1e-9; 2],
                        &mut rows,
                    )
                    .unwrap();
            }
            let solved = crate::constraint_response::solve(&rows, 1024).unwrap();
            for (entity, impulse) in crate::constraint_response::impulses(&rows, &solved).unwrap() {
                let body = if entity == Entity::from_bits(1) {
                    &mut a
                } else {
                    &mut b
                };
                crate::constraint_response::apply_body_impulse(body, &inertia, impulse).unwrap();
            }
        }
        panic!("redundant native hard rows did not close");
    }

    #[test]
    fn residual_measures_hard_rows_and_preserves_free_slider_travel() {
        let projection = prepared(
            |a, b| {
                PrismaticJoint::new(a, b)
                    .with_slider_axis(DVec3::Y)
                    .with_limits(-0.055, 0.065)
            },
            [DQuat::IDENTITY; 2],
        );
        let a = SolverBody::default();
        let mut b = SolverBody {
            delta_position: DVec3::Y * 0.04,
            ..default()
        };
        assert_eq!(projection.residual([&a, &b]).unwrap(), (0.0, 0.0));
        b.delta_position = DVec3::new(0.02, 0.08, 0.0);
        let error = projection.residual([&a, &b]).unwrap();
        assert!((error.0 - 0.02).abs() < 1e-12);
        b.delta_rotation = Rotation(DQuat::from_rotation_z(0.01));
        assert!((projection.residual([&a, &b]).unwrap().1 - 0.01).abs() < 1e-12);
    }

    #[test]
    fn projection_preserves_soft_translation_while_closing_hard_rotation() {
        let inertia = inertia();
        for angle_compliance in [1.0, 0.0] {
            let mut projection = prepared(
                |a, b| {
                    FixedJoint::new(a, b)
                        .with_point_compliance(1.0)
                        .with_angle_compliance(angle_compliance)
                },
                [DQuat::IDENTITY; 2],
            );
            let mut a = SolverBody::default();
            let mut b = SolverBody {
                delta_position: DVec3::X,
                delta_rotation: Rotation(DQuat::from_rotation_y(0.2)),
                ..default()
            };
            let rotation = b.delta_rotation;
            projection
                .project_once([&mut a, &mut b], [&inertia, &inertia])
                .unwrap();
            assert_eq!(a.delta_position, DVec3::ZERO);
            assert_eq!(b.delta_position, DVec3::X);
            if angle_compliance == 0.0 {
                assert!(
                    crate::joint::rotation_displacement(a.delta_rotation.0, b.delta_rotation.0)
                        .unwrap()
                        .length()
                        < 1e-14
                );
            } else {
                assert_eq!(a.delta_rotation, Rotation::IDENTITY);
                assert_eq!(b.delta_rotation, rotation);
            }
        }
    }

    #[test]
    fn projection_uses_world_joint_frames_for_noncommuting_rotations() {
        let basis1 = DQuat::from_rotation_x(0.5);
        let basis2 = DQuat::from_rotation_z(-0.4);
        let rotation1 = DQuat::from_euler(bevy::math::EulerRot::XYZ, 0.2, -0.7, 0.3);
        let rotation2 = rotation1 * basis1 * basis2.inverse();
        let mut projection = prepared(
            |a, b| {
                FixedJoint::new(a, b)
                    .with_local_basis1(basis1)
                    .with_local_basis2(basis2)
            },
            [rotation1, rotation2],
        );
        let mut a = SolverBody {
            delta_rotation: Rotation(DQuat::from_rotation_y(0.3)),
            ..default()
        };
        let mut b = SolverBody {
            delta_rotation: Rotation(DQuat::from_rotation_x(-0.2)),
            ..default()
        };
        let inertia = inertia();
        projection
            .project_once([&mut a, &mut b], [&inertia, &inertia])
            .unwrap();
        let first = a.delta_rotation.0 * rotation1 * basis1;
        let second = b.delta_rotation.0 * rotation2 * basis2;
        assert!(
            crate::joint::rotation_displacement(first, second)
                .unwrap()
                .length()
                < 1e-14
        );
    }

    #[test]
    fn projection_does_not_reintegrate_a_revolute_motor() {
        let mut projection = prepared(
            |a, b| {
                RevoluteJoint::new(a, b)
                    .with_hinge_axis(DVec3::Y)
                    .with_motor(
                        AngularMotor::new(MotorModel::ForceBased {
                            stiffness: 1000.0,
                            damping: 20.0,
                        })
                        .with_target_position(0.8),
                    )
            },
            [DQuat::IDENTITY; 2],
        );
        let mut a = SolverBody::default();
        let mut b = SolverBody {
            delta_rotation: Rotation(DQuat::from_rotation_y(0.3)),
            angular_velocity: DVec3::Y * 2.0,
            ..default()
        };
        let inertia = inertia();
        for _ in 0..20 {
            projection
                .project_once([&mut a, &mut b], [&inertia, &inertia])
                .unwrap();
        }
        assert_eq!(a.delta_rotation, Rotation::IDENTITY);
        assert_eq!(b.delta_rotation, Rotation(DQuat::from_rotation_y(0.3)));
        assert_eq!(a.angular_velocity, DVec3::ZERO);
        assert_eq!(b.angular_velocity, DVec3::Y * 2.0);
        assert_eq!(projection.rotation_lagrange, DVec3::ZERO);
    }

    #[test]
    fn projection_separates_slider_alignment_and_travel_compliance() {
        let mut projection = prepared(
            |a, b| {
                PrismaticJoint::new(a, b)
                    .with_align_compliance(1.0)
                    .with_limits(-0.2, 0.2)
            },
            [DQuat::IDENTITY; 2],
        );
        let mut a = SolverBody::default();
        let mut b = SolverBody {
            delta_position: DVec3::new(1.0, 0.4, 0.0),
            ..default()
        };
        let inertia = inertia();
        projection
            .project_once([&mut a, &mut b], [&inertia, &inertia])
            .unwrap();
        let separation = b.delta_position - a.delta_position;
        assert!((separation.x - 0.2).abs() < 1e-14);
        assert_eq!(separation.y, 0.4);
        assert_eq!(separation.z, 0.0);
    }

    #[test]
    fn projection_closes_distance_and_spherical_limits() {
        let inertia = inertia();
        let mut distance = prepared(
            |a, b| DistanceJoint::new(a, b).with_limits(1.0, 1.0),
            [DQuat::IDENTITY; 2],
        );
        let mut a = SolverBody::default();
        let mut b = SolverBody {
            delta_position: DVec3::X * 2.0,
            ..default()
        };
        distance
            .project_once([&mut a, &mut b], [&inertia, &inertia])
            .unwrap();
        assert!(((b.delta_position - a.delta_position).length() - 1.0).abs() < 1e-14);
        for (axis, swing, twist) in [
            (DVec3::X, Some(AngleLimit::new(-0.1, 0.1)), None),
            (DVec3::Y, None, Some(AngleLimit::new(-0.1, 0.1))),
        ] {
            let mut spherical = prepared(
                |a, b| {
                    let mut joint = SphericalJoint::new(a, b);
                    joint.swing_limit = swing;
                    joint.twist_limit = twist;
                    joint
                },
                [DQuat::IDENTITY; 2],
            );
            let mut a = SolverBody::default();
            let mut b = SolverBody {
                delta_rotation: Rotation(DQuat::from_axis_angle(axis, 0.3)),
                ..default()
            };
            spherical
                .project_once([&mut a, &mut b], [&inertia, &inertia])
                .unwrap();
            let angle = crate::joint::rotation_displacement(a.delta_rotation.0, b.delta_rotation.0)
                .unwrap();
            assert!((angle - axis * 0.1).length() < 1e-14, "{angle:?}");
        }
    }

    #[test]
    fn projection_rejects_unprepared_or_ambiguous_geometry_without_partial_writes() {
        let inertia = inertia();
        let mut a = SolverBody::default();
        let mut b = SolverBody {
            delta_position: DVec3::X,
            ..default()
        };
        assert!(
            JointProjection::default()
                .project_once([&mut a, &mut b], [&inertia, &inertia])
                .is_err()
        );
        assert_eq!(a.delta_position, DVec3::ZERO);
        assert_eq!(b.delta_position, DVec3::X);
        let mut distance = prepared(
            |a, b| DistanceJoint::new(a, b).with_limits(1.0, 1.0),
            [DQuat::IDENTITY; 2],
        );
        b.delta_position = DVec3::ZERO;
        assert!(
            distance
                .project_once([&mut a, &mut b], [&inertia, &inertia])
                .is_err()
        );
        assert_eq!(a.delta_position, DVec3::ZERO);
        assert_eq!(b.delta_position, DVec3::ZERO);
    }
}
