//! Hard geometric closure composed from public joint frames and impulse kernels.

use avian3d::dynamics::joints::EntityConstraint;
use avian3d::dynamics::solver::{
    solver_body::{SolverBody, SolverBodyInertia},
    xpbd::{AngularConstraint, PositionConstraint, joints::PointConstraintShared},
};
use avian3d::prelude::*;
use bevy::{
    math::{DMat3, DQuat, DVec3},
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

    pub(crate) fn solve(
        &mut self,
        bodies: [&mut SolverBody; 2],
        inertias: [&SolverBodyInertia; 2],
    ) -> Result<(), &'static str> {
        if !self.prepared {
            return Err("joint projection has no admitted geometry");
        }
        let [first, second] = bodies;
        let [mut a, mut b] = [first.clone(), second.clone()];
        let [mut linear, mut angular] = [DVec3::ZERO; 2];
        for row in self.rows.into_iter().flatten() {
            let levers = [
                a.delta_rotation * self.point.world_r1,
                b.delta_rotation * self.point.world_r2,
            ];
            let separation = (b.delta_position - a.delta_position)
                + (levers[1] - levers[0])
                + self.point.center_difference;
            let mut position_error = None;
            let mut angle_error = None;
            let mut position_space = DMat3::IDENTITY;
            let mut angle_space = DMat3::IDENTITY;
            match row {
                GeometryRow::Anchors => position_error = Some(separation),
                GeometryRow::Distance(limit) => {
                    let length = separation.length();
                    if length == 0.0 && limit.min > 0.0 {
                        return Err("separated distance anchors have no resolved direction");
                    }
                    if length != 0.0 {
                        position_space = outer(separation / length);
                        position_error = Some(
                            separation * ((length - length.clamp(limit.min, limit.max)) / length),
                        );
                    }
                }
                _ => {
                    let bases = self
                        .bases
                        .ok_or("angular joint projection requires prepared bases")?;
                    let frames = [a.delta_rotation.0 * bases[0], b.delta_rotation.0 * bases[1]];
                    match row {
                        GeometryRow::Orientation => {
                            angle_error =
                                Some(crate::joint::rotation_displacement(frames[0], frames[1])?)
                        }
                        GeometryRow::SliderAlignment(axis) => {
                            let axis = unit(frames[0] * axis)?;
                            position_space = DMat3::IDENTITY - outer(axis);
                            position_error = Some(separation - axis * separation.dot(axis));
                        }
                        GeometryRow::SliderLimit(axis, limit) => {
                            position_space = outer(unit(frames[0] * axis)?);
                            position_error =
                                Some(-limit.compute_correction_along_axis(
                                    separation,
                                    unit(frames[0] * axis)?,
                                ))
                        }
                        GeometryRow::AlignedAxes(axis) => {
                            angle_space = DMat3::IDENTITY - outer(unit(frames[0] * axis)?);
                            angle_error =
                                Some(axis_error(frames[0] * axis, frames[1] * axis, None)?)
                        }
                        GeometryRow::Angle(axis, limit) => {
                            angle_space = outer(unit(frames[0] * axis)?);
                            let perpendicular = axis.any_orthonormal_vector();
                            angle_error = Some(angle_error_about(
                                unit(frames[0] * axis)?,
                                frames[0] * perpendicular,
                                frames[1] * perpendicular,
                                limit,
                            )?);
                        }
                        GeometryRow::Swing(axis, limit) => {
                            angle_error =
                                Some(axis_error(frames[0] * axis, frames[1] * axis, Some(limit))?);
                        }
                        GeometryRow::Twist(axis, limit) => {
                            let perpendicular = axis.any_orthonormal_vector();
                            let normal = unit(frames[0] * axis + frames[1] * axis)?;
                            angle_error = Some(angle_error_about(
                                normal,
                                frames[0] * perpendicular,
                                frames[1] * perpendicular,
                                limit,
                            )?);
                        }
                        _ => unreachable!(),
                    }
                }
            }
            if let Some(error) = position_error {
                linear +=
                    project_position([&mut a, &mut b], inertias, levers, error, position_space)?;
            }
            if let Some(error) = angle_error {
                if matches!(row, GeometryRow::Swing(..) | GeometryRow::Twist(..))
                    && error.length_squared() > 0.0
                {
                    angle_space = outer(error.normalize());
                }
                angular -= project_angle([&mut a, &mut b], inertias, error, angle_space)?;
            }
        }
        if ![&a, &b].into_iter().all(|body| {
            body.delta_position.is_finite()
                && crate::avian_backend_rotation_is_valid(body.delta_rotation.0)
        }) || !(self.position_lagrange + linear).is_finite()
            || !(self.rotation_lagrange + angular).is_finite()
        {
            return Err("joint projection result is not a finite rigid state");
        }
        *first = a;
        *second = b;
        self.position_lagrange += linear;
        self.rotation_lagrange += angular;
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

struct ProjectionKernel;
impl PositionConstraint for ProjectionKernel {}
impl AngularConstraint for ProjectionKernel {}

fn project_position(
    bodies: [&mut SolverBody; 2],
    inertias: [&SolverBodyInertia; 2],
    levers: [DVec3; 2],
    error: DVec3,
    space: DMat3,
) -> Result<DVec3, &'static str> {
    if !error.is_finite() {
        return Err("joint positional error is not finite");
    }
    let magnitude = error.length();
    if !magnitude.is_finite() {
        return Err("joint positional error magnitude is not representable");
    }
    if magnitude == 0.0 {
        return Ok(DVec3::ZERO);
    }
    // Solve all constrained channels together. A scalar inverse mass along
    // the current error leaves cross-axis errors for anisotropic bodies.
    let response = |direction: DVec3| {
        inertias
            .into_iter()
            .zip(levers)
            .fold(DVec3::ZERO, |sum, (inertia, lever)| {
                sum + inertia.effective_inv_mass() * direction
                    + (inertia.effective_inv_angular_inertia() * lever.cross(direction))
                        .cross(lever)
            })
    };
    let matrix = DMat3::from_cols(response(DVec3::X), response(DVec3::Y), response(DVec3::Z));
    let impulse = block_correction_impulse(error, matrix, space)?;
    let [a, b] = bodies;
    ProjectionKernel.apply_positional_impulse(
        a,
        b,
        inertias[0],
        inertias[1],
        impulse,
        levers[0],
        levers[1],
    );
    Ok(impulse)
}

fn project_angle(
    bodies: [&mut SolverBody; 2],
    inertias: [&SolverBodyInertia; 2],
    error: DVec3,
    space: DMat3,
) -> Result<DVec3, &'static str> {
    if !error.is_finite() {
        return Err("joint angular error is not finite");
    }
    let magnitude = error.length();
    if !magnitude.is_finite() {
        return Err("joint angular error magnitude is not representable");
    }
    if magnitude == 0.0 {
        return Ok(DVec3::ZERO);
    }
    let response = |direction| {
        inertias.into_iter().fold(DVec3::ZERO, |sum, inertia| {
            sum + inertia.effective_inv_angular_inertia() * direction
        })
    };
    let matrix = DMat3::from_cols(response(DVec3::X), response(DVec3::Y), response(DVec3::Z));
    let impulse = block_correction_impulse(error, matrix, space)?;
    let [a, b] = bodies;
    ProjectionKernel.apply_angular_impulse(
        a,
        b,
        inertias[0].effective_inv_angular_inertia(),
        inertias[1].effective_inv_angular_inertia(),
        impulse,
    );
    Ok(impulse)
}

fn outer(axis: DVec3) -> DMat3 {
    DMat3::from_cols(axis * axis.x, axis * axis.y, axis * axis.z)
}

/// Mass-metric projection in the row's authored constrained subspace. The
/// maintained symmetric eigensolver supplies the pseudoinverse: locked/free
/// null modes receive no impulse, preserving native locked-axis semantics.
fn block_correction_impulse(
    error: DVec3,
    response: DMat3,
    space: DMat3,
) -> Result<DVec3, &'static str> {
    if !response.is_finite() || !space.is_finite() {
        return Err("joint projection response is invalid");
    }
    let scale = response
        .x_axis
        .abs()
        .max(response.y_axis.abs())
        .max(response.z_axis.abs())
        .max_element();
    if scale == 0.0 {
        return Ok(DVec3::ZERO);
    }
    // Free channels have zero RHS. A positive complementary block preserves
    // that null-space result without introducing repeated artificial zero roots.
    let matrix = space * response * space + (DMat3::IDENTITY - space) * scale;
    Ok(space * crate::joint::responsive_impulse(matrix, space * error)?)
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
    fn anisotropic_block_resolves_coupled_channels_and_preserves_free_axis() {
        let response = DMat3::from_cols(
            DVec3::new(2.0, 0.6, 0.2),
            DVec3::new(0.6, 1.0, 0.3),
            DVec3::new(0.2, 0.3, 3.0),
        );
        let error = DVec3::new(0.1, -0.2, 0.3);
        let impulse = block_correction_impulse(error, response, DMat3::IDENTITY).unwrap();
        assert!((response * impulse - error).length() < 1e-12);
        let axis = DVec3::new(0.2, 0.7, -0.4).normalize();
        let constrained = DMat3::IDENTITY - outer(axis);
        let impulse = block_correction_impulse(error, response, constrained).unwrap();
        assert!(impulse.dot(axis).abs() < 1e-12);
        assert!((constrained * (response * impulse - error)).length() < 1e-12);
        let impulse = block_correction_impulse(error, response, outer(axis)).unwrap();
        assert!(impulse.cross(axis).length() < 1e-12);
        assert!((outer(axis) * (response * impulse - error)).length() < 1e-12);
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
                .solve([&mut a, &mut b], [&inertia, &inertia])
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
            .solve([&mut a, &mut b], [&inertia, &inertia])
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
                .solve([&mut a, &mut b], [&inertia, &inertia])
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
            .solve([&mut a, &mut b], [&inertia, &inertia])
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
            .solve([&mut a, &mut b], [&inertia, &inertia])
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
                .solve([&mut a, &mut b], [&inertia, &inertia])
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
                .solve([&mut a, &mut b], [&inertia, &inertia])
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
                .solve([&mut a, &mut b], [&inertia, &inertia])
                .is_err()
        );
        assert_eq!(a.delta_position, DVec3::ZERO);
        assert_eq!(b.delta_position, DVec3::ZERO);
    }
}
