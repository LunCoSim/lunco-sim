//! Reusable joint mechanics at the Avian boundary.
//!
//! This module contains the physical actuator description and small mechanical
//! projections shared by the USD bridge, mobility realization, and the
//! co-simulation force application system. It does not own port resolution or
//! command transport.

use avian3d::dynamics::solver::solver_body::{SolverBody, SolverBodyInertia};
use avian3d::math::SymmetricMatrix;
use avian3d::prelude::{JointDamping, RevoluteJoint};
use bevy::math::{DMat3, DQuat, DVec3};
use bevy::prelude::*;

/// The port name a revolute joint exposes in both directions.
pub const JOINT_ANGLE_PORT: &str = "angle";

/// The port name a revolute joint exposes for its measured coordinate rate.
pub const JOINT_ANGULAR_VELOCITY_PORT: &str = "angular_velocity";

/// The port name a prismatic joint exposes in both directions.
pub const JOINT_DISPLACEMENT_PORT: &str = "displacement";

/// The port name a prismatic joint exposes for its slide rate (m/s).
pub const JOINT_VELOCITY_PORT: &str = "velocity";

/// The port name a prismatic joint exposes for its completed net force on body2 along the slider axis (N).
pub const JOINT_FORCE_PORT: &str = "force";

/// A solved scalar torque applied across a revolute joint.
///
/// The scalar is a physical torque from an authored runtime port. The joint
/// coordinate sign maps it to equal and opposite world torques on the two
/// bodies; Avian then integrates the bodies and enforces the revolute
/// constraint. This is a reusable mechanical boundary, not a motor model:
/// motors, brakes, hydraulic rotary actuators, and other domains can publish
/// the scalar through the same port surface. The same boundary publishes the
/// solved joint-coordinate speed through `speed_port_entity`.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component, Default)]
pub struct JointTorqueActuator {
    /// Runtime port carrying solved torque in the joint's positive coordinate.
    pub port_entity: Entity,
    /// Runtime port receiving the measured joint-coordinate speed, rad/s.
    pub speed_port_entity: Entity,
    /// Passive opposing torque when the owning vessel's brake command is active.
    pub brake_torque: f64,
    /// Rotational inertia at the actuator boundary, kg m². It bounds the
    /// braking impulse for one fixed step so a brake cannot integrate through
    /// zero and reverse the joint every tick.
    pub rotational_inertia: f64,
    /// Mapping from the joint coordinate to body-2's world axis.
    pub drive_sign: f64,
}

impl Default for JointTorqueActuator {
    fn default() -> Self {
        Self {
            port_entity: Entity::PLACEHOLDER,
            speed_port_entity: Entity::PLACEHOLDER,
            brake_torque: 0.0,
            rotational_inertia: 0.0,
            drive_sign: 1.0,
        }
    }
}

/// Return the world-space axis of a revolute joint's positive coordinate.
///
/// Avian defines the coordinate axis in the joint frame: `hinge_axis` is
/// rotated by `frame1.basis` before it is expressed in body 1's world
/// orientation. Keeping that projection here makes every mechanical boundary
/// use the solver's actual axis, including a runtime steering change to the
/// joint frame.
pub fn revolute_hinge_axis_world(joint: &RevoluteJoint, body1_rotation: DQuat) -> Option<DVec3> {
    let local_axis = joint.local_hinge_axis1()?;
    (body1_rotation * local_axis).try_normalize()
}

/// Measure the signed revolute coordinate and its positive world-space axis.
///
/// Both authored joint bases participate in the measurement. The perpendicular
/// frame vectors follow Avian's motor convention; `atan2` retains f64 precision
/// for small angles. Unresolved or invalid rigid frames have no measurement.
pub fn revolute_coordinate(
    joint: &RevoluteJoint,
    body1_rotation: DQuat,
    body2_rotation: DQuat,
) -> Option<(f64, DVec3)> {
    let basis1 = joint.local_basis1()?;
    let basis2 = joint.local_basis2()?;
    if ![body1_rotation, body2_rotation, basis1, basis2]
        .into_iter()
        .all(crate::avian_backend_rotation_is_valid)
        || !joint.hinge_axis.is_finite()
        || !joint.hinge_axis.is_normalized()
    {
        return None;
    }
    angular_frame_coordinate(
        body1_rotation * basis1,
        body2_rotation * basis2,
        joint.hinge_axis,
    )
}

pub(crate) fn angular_frame_coordinate(
    frame1: DQuat,
    frame2: DQuat,
    local_axis: DVec3,
) -> Option<(f64, DVec3)> {
    if ![frame1, frame2]
        .into_iter()
        .all(crate::avian_backend_rotation_is_valid)
        || !local_axis.is_finite()
        || !local_axis.is_normalized()
    {
        return None;
    }
    let axis = (frame1 * local_axis).try_normalize()?;
    let perpendicular = local_axis.any_orthonormal_vector();
    let first = frame1 * perpendicular;
    let second = frame2 * perpendicular;
    let angle = first.cross(second).dot(axis).atan2(first.dot(second));
    angle.is_finite().then_some((angle, axis))
}

/// Shortest world-space angular displacement between admitted rigid rotations.
///
/// Evaluating the quaternion logarithm with `atan2` retains small increments
/// and is invariant to quaternion hemisphere and admitted norm drift.
pub fn rotation_displacement(start: DQuat, end: DQuat) -> Result<DVec3, &'static str> {
    if !crate::avian_backend_rotation_is_valid(start)
        || !crate::avian_backend_rotation_is_valid(end)
    {
        return Err("angular displacement requires finite unit rotations");
    }
    let relative = end * start.inverse();
    let hemisphere = if relative.w < 0.0 { -1.0 } else { 1.0 };
    let vector = hemisphere * DVec3::new(relative.x, relative.y, relative.z);
    let length = vector.x.hypot(vector.y).hypot(vector.z);
    if length == 0.0 {
        return Ok(DVec3::ZERO);
    }
    Ok(vector * (2.0 * length.atan2(hemisphere * relative.w) / length))
}

/// Integrate passive relative-velocity damping for one physics substep.
///
/// The authored coefficients are decay rates in 1/s. The point-velocity and
/// angular-velocity flows each use exponential decay, applied in that order.
/// `levers` locate one common world-space damping point from each body's COM.
/// Equal and opposite impulses, including their moments at that point,
/// distribute the change through the full effective inertia response;
/// locked axes and dominant endpoints cannot acquire velocity. This is not a
/// spring drive and must run only once per substep, outside position closure.
/// Invalid input or an unrepresentable result returns before either write.
pub(crate) fn integrate_joint_damping(
    bodies: [&mut SolverBody; 2],
    inertias: [&SolverBodyInertia; 2],
    levers: [DVec3; 2],
    damping: JointDamping,
    dt: f64,
) -> Result<(), &'static str> {
    let [first, second] = bodies;
    if ![dt, damping.linear, damping.angular]
        .into_iter()
        .all(|x| x.is_finite() && x >= 0.0)
        || ![
            levers[0],
            levers[1],
            first.linear_velocity,
            second.linear_velocity,
            first.angular_velocity,
            second.angular_velocity,
        ]
        .into_iter()
        .all(DVec3::is_finite)
    {
        return Err("joint damping requires finite velocities, nonnegative rates and duration");
    }
    let [mut first_inertia, mut second_inertia] = inertias;
    match first_inertia.dominance().cmp(&second_inertia.dominance()) {
        std::cmp::Ordering::Greater => first_inertia = &SolverBodyInertia::DUMMY,
        std::cmp::Ordering::Less => second_inertia = &SolverBodyInertia::DUMMY,
        std::cmp::Ordering::Equal => {}
    }
    let mass1 = first_inertia.effective_inv_mass();
    let mass2 = second_inertia.effective_inv_mass();
    let angular1 = first_inertia.effective_inv_angular_inertia();
    let angular2 = second_inertia.effective_inv_angular_inertia();
    if ![mass1, mass2]
        .into_iter()
        .all(|x| x.is_finite() && x.cmpge(DVec3::ZERO).all())
    {
        return Err("joint damping requires nonnegative finite inverse masses");
    }
    // Avian locks rotations in world-coordinate axes. Invert only their
    // movable subspace; a singular tensor with another nullspace is invalid
    // at this boundary rather than a reason to discard its free-axis motion.
    inverse_unlocked_inertia(angular1)?;
    inverse_unlocked_inertia(angular2)?;
    let angular_sum_inverse = inverse_unlocked_inertia(angular1 + angular2)?;
    if dt == 0.0 || (damping.linear == 0.0 && damping.angular == 0.0) {
        return Ok(());
    }
    let fraction = |rate: f64| -(-rate * dt).exp_m1();
    let mass_sum = mass1 + mass2;
    if !mass_sum.is_finite() {
        return Err("joint damping combined inverse mass is not representable");
    }
    let linear_fraction = fraction(damping.linear);
    let [lever1, lever2] = levers;
    let relative = second.velocity_at_point(lever2) - first.velocity_at_point(lever1);
    let point_response = |axis: DVec3| {
        mass_sum * axis
            + (angular1 * lever1.cross(axis)).cross(lever1)
            + (angular2 * lever2.cross(axis)).cross(lever2)
    };
    let linear_impulse = if linear_fraction == 0.0 {
        DVec3::ZERO
    } else {
        let response = DMat3::from_cols(
            point_response(DVec3::X),
            point_response(DVec3::Y),
            point_response(DVec3::Z),
        );
        responsive_impulse(response, relative * linear_fraction)?
    };
    let point_angular1 = first.angular_velocity + angular1 * lever1.cross(linear_impulse);
    let point_angular2 = second.angular_velocity - angular2 * lever2.cross(linear_impulse);
    let angular_impulse =
        angular_sum_inverse * ((point_angular2 - point_angular1) * fraction(damping.angular));
    let linear1 = first.linear_velocity + mass1 * linear_impulse;
    let linear2 = second.linear_velocity - mass2 * linear_impulse;
    let velocity1 = point_angular1 + angular1 * angular_impulse;
    let velocity2 = point_angular2 - angular2 * angular_impulse;
    if ![
        linear_impulse,
        angular_impulse,
        linear1,
        linear2,
        velocity1,
        velocity2,
    ]
    .into_iter()
    .all(DVec3::is_finite)
    {
        return Err("joint damping impulse or resulting velocity is not representable");
    }
    first.linear_velocity = linear1;
    second.linear_velocity = linear2;
    first.angular_velocity = velocity1;
    second.angular_velocity = velocity2;
    Ok(())
}

fn responsive_impulse(response: DMat3, change: DVec3) -> Result<DVec3, &'static str> {
    const RANK_TOLERANCE: f64 = 32.0 * f64::EPSILON;
    if !response.is_finite() || !change.is_finite() {
        return Err("joint damping point response is not representable");
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
    // The maintained f64 symmetric eigensolver supplies a pseudoinverse for
    // constrained point responses, whose nullspace need not follow world axes.
    use avian3d::parry::utils::SymmetricEigen3;
    let normalized = response / scale;
    let values = SymmetricEigen3::eigenvalues(normalized);
    if !values.is_finite() || values.min_element() < -RANK_TOLERANCE {
        return Err("joint damping point response is not positive semidefinite");
    }
    // eigenvector1 requires an isolated root. Start at the end separated by
    // the larger gap, then let the public second-vector kernel handle the
    // remaining eigenspace, including its repeated roots.
    let values = if values.z - values.y > values.y - values.x {
        DVec3::new(values.z, values.y, values.x)
    } else {
        values
    };
    let first = SymmetricEigen3::eigenvector1(normalized, values.x);
    let second = SymmetricEigen3::eigenvector2(normalized, first, values.y);
    let third = SymmetricEigen3::eigenvector3(first, second);
    let vectors = DMat3::from_cols(first, second, third);
    if !vectors.is_finite() {
        return Err("joint damping point response eigenspace is not representable");
    }
    let coordinates = vectors.transpose() * change;
    let impulse = DVec3::from_array(std::array::from_fn(|i| {
        if values[i] <= RANK_TOLERANCE {
            0.0
        } else {
            coordinates[i] / values[i] / scale
        }
    }));
    let impulse = vectors * impulse;
    if !impulse.is_finite() {
        return Err("joint damping point impulse is not representable");
    }
    Ok(impulse)
}

fn inverse_unlocked_inertia(matrix: SymmetricMatrix) -> Result<SymmetricMatrix, &'static str> {
    let diagonal = matrix.diagonal();
    if !matrix.is_finite() || !diagonal.cmpge(DVec3::ZERO).all() {
        return Err("joint damping requires a finite positive inverse inertia on free axes");
    }
    let locked = diagonal.cmpeq(DVec3::ZERO);
    if (locked.x && (matrix.m01 != 0.0 || matrix.m02 != 0.0))
        || (locked.y && (matrix.m01 != 0.0 || matrix.m12 != 0.0))
        || (locked.z && (matrix.m02 != 0.0 || matrix.m12 != 0.0))
    {
        return Err("joint damping inverse inertia has an invalid locked axis");
    }
    let scale = diagonal.max_element();
    if scale == 0.0 {
        return Ok(SymmetricMatrix::ZERO);
    }
    let lock_diagonal = DVec3::select(locked, DVec3::ONE, DVec3::ZERO);
    let lock_matrix = SymmetricMatrix::from_diagonal(lock_diagonal);
    let normalized = matrix / scale + lock_matrix;
    if normalized.m00 <= 0.0
        || normalized.m00 * normalized.m11 - normalized.m01 * normalized.m01 <= 0.0
        || normalized.determinant() <= 0.0
    {
        return Err("joint damping inverse inertia is singular or not positive on free axes");
    }
    let inverse = (normalized.inverse() - lock_matrix) / scale;
    if !inverse.is_finite() {
        return Err("joint damping inverse inertia cannot be inverted at f64 precision");
    }
    Ok(inverse)
}

/// Return the first revolute joint in `root`'s subtree.
pub fn joint_angle_holder(world: &World, root: Entity) -> Option<Entity> {
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if world.get::<RevoluteJoint>(entity).is_some() {
            return Some(entity);
        }
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    None
}

/// Resolve the physical brake torque for one fixed step.
///
/// The authored value is a torque limit. Near zero speed, applying that full
/// limit for an entire step would overshoot the stop and reverse the joint;
/// the impulse needed to reach zero is the physically valid bound for this
/// discrete projection.
#[inline]
pub fn bounded_brake_torque(
    max_torque: f64,
    rotational_inertia: f64,
    coordinate_speed: f64,
    dt: f64,
) -> f64 {
    if !max_torque.is_finite()
        || max_torque <= 0.0
        || !rotational_inertia.is_finite()
        || rotational_inertia <= 0.0
        || !coordinate_speed.is_finite()
        || !dt.is_finite()
        || dt <= 0.0
    {
        return 0.0;
    }
    -coordinate_speed.signum() * max_torque.min(rotational_inertia * coordinate_speed.abs() / dt)
}

#[cfg(test)]
mod tests {
    use super::{
        bounded_brake_torque, integrate_joint_damping, revolute_coordinate, rotation_displacement,
    };
    use avian3d::dynamics::solver::solver_body::{SolverBody, SolverBodyInertia};
    use avian3d::math::SymmetricMatrix;
    use avian3d::prelude::{JointDamping, LockedAxes, RevoluteJoint};
    use bevy::{
        math::{DQuat, DVec3, EulerRot},
        prelude::*,
    };

    #[test]
    fn revolute_coordinate_uses_both_joint_frames() {
        let mut world = World::new();
        let first = world.spawn_empty().id();
        let second = world.spawn_empty().id();
        let basis1 = DQuat::from_rotation_z(0.4);
        let basis2 = DQuat::from_rotation_x(-0.6);
        let body1 = DQuat::from_euler(EulerRot::XYZ, 0.2, -0.7, 0.3);
        let joint = RevoluteJoint::new(first, second)
            .with_hinge_axis(DVec3::Y)
            .with_local_basis1(basis1)
            .with_local_basis2(basis2);
        let expected_axis = body1 * basis1 * DVec3::Y;
        for angle in [0.0, 2e-10, -2e-10, 0.3, -0.3, 3.0, -3.0] {
            let body2 = body1 * basis1 * DQuat::from_rotation_y(angle) * basis2.inverse();
            for sign in [-1.0, 1.0] {
                let (measured, axis) = revolute_coordinate(&joint, body1, body2 * sign).unwrap();
                assert!((measured - angle).abs() < 2e-15, "{measured} {angle}");
                assert!((axis - expected_axis).length() < 1e-15);
            }
        }
        for invalid in [
            DQuat::NAN,
            DQuat::from_xyzw(0.0, 0.0, 0.0, 0.0),
            body1 * 2.0,
        ] {
            assert!(revolute_coordinate(&joint, invalid, body1).is_none());
            assert!(revolute_coordinate(&joint, body1, invalid).is_none());
        }
        let invalid_axis = joint.clone().with_hinge_axis(DVec3::ZERO);
        assert!(revolute_coordinate(&invalid_axis, body1, body1).is_none());
    }

    #[test]
    fn rotation_displacement_retains_small_increments_and_rejects_invalid_frames() {
        let start = DQuat::from_euler(EulerRot::XYZ, 0.2, -0.7, 0.3);
        let axis = DVec3::new(0.3, -0.4, 0.5).normalize();
        for angle in [0.0, 2e-10, -2e-10, 0.3, -0.3, 3.0, -3.0] {
            let end = DQuat::from_scaled_axis(axis * angle) * start;
            for sign in [-1.0, 1.0] {
                let measured = rotation_displacement(start, end * (sign * (1.0 - 1e-12))).unwrap();
                assert!((measured - axis * angle).length() < 2e-15);
            }
        }
        let invalid = DQuat::from_xyzw(0.0, 0.0, 0.0, 0.0);
        assert!(rotation_displacement(invalid, start).is_err());
        assert!(rotation_displacement(start, DQuat::NAN).is_err());
    }

    #[test]
    fn joint_damping_conserves_momentum_and_integrates_the_decay_rate() {
        // Distinct anisotropic inertias with nonzero off-diagonal terms ensure
        // angular impulses use the full tensor rather than a mass or diagonal.
        let inverse1 = SymmetricMatrix::new(2.0, 0.3, 0.1, 3.0, -0.2, 4.0);
        let inverse2 = SymmetricMatrix::new(5.0, -0.4, 0.2, 2.0, 0.1, 3.0);
        let inertia1 = SolverBodyInertia::new(0.25, inverse1, LockedAxes::new(), 0, true);
        let inertia2 = SolverBodyInertia::new(2.0, inverse2, LockedAxes::new(), 0, true);
        let first = SolverBody {
            linear_velocity: DVec3::new(3.0, -1.0, 2.0),
            angular_velocity: DVec3::new(0.3, 2.0, -0.5),
            ..default()
        };
        let second = SolverBody {
            linear_velocity: DVec3::new(-2.0, 0.5, 7.0),
            angular_velocity: DVec3::new(-1.0, 0.2, 4.0),
            ..default()
        };
        let damping = JointDamping {
            linear: 7.0,
            angular: 11.0,
        };
        let dt = 0.05;
        let momentum =
            |a: &SolverBody, b: &SolverBody| 4.0 * a.linear_velocity + 0.5 * b.linear_velocity;
        let angular_momentum = |a: &SolverBody, b: &SolverBody| {
            inverse1.inverse() * a.angular_velocity + inverse2.inverse() * b.angular_velocity
        };
        let energy = |a: &SolverBody, b: &SolverBody| {
            0.5 * (4.0 * a.linear_velocity.length_squared()
                + 0.5 * b.linear_velocity.length_squared()
                + a.angular_velocity
                    .dot(inverse1.inverse() * a.angular_velocity)
                + b.angular_velocity
                    .dot(inverse2.inverse() * b.angular_velocity))
        };
        let (mut a, mut b) = (first.clone(), second.clone());
        integrate_joint_damping(
            [&mut a, &mut b],
            [&inertia1, &inertia2],
            [DVec3::ZERO; 2],
            damping,
            dt,
        )
        .unwrap();
        assert!((momentum(&a, &b) - momentum(&first, &second)).length() < 2e-14);
        assert!((angular_momentum(&a, &b) - angular_momentum(&first, &second)).length() < 2e-14);
        assert!(energy(&a, &b) < energy(&first, &second));
        assert!(
            (b.linear_velocity
                - a.linear_velocity
                - (second.linear_velocity - first.linear_velocity) * (-damping.linear * dt).exp())
            .length()
                < 3e-15
        );
        assert!(
            (b.angular_velocity
                - a.angular_velocity
                - (second.angular_velocity - first.angular_velocity)
                    * (-damping.angular * dt).exp())
            .length()
                < 3e-15
        );
        let (mut split_a, mut split_b) = (first, second);
        for _ in 0..10 {
            integrate_joint_damping(
                [&mut split_a, &mut split_b],
                [&inertia1, &inertia2],
                [DVec3::ZERO; 2],
                damping,
                dt / 10.0,
            )
            .unwrap();
        }
        assert!((split_a.linear_velocity - a.linear_velocity).length() < 2e-14);
        assert!((split_b.linear_velocity - b.linear_velocity).length() < 2e-14);
        assert!((split_a.angular_velocity - a.angular_velocity).length() < 2e-14);
        assert!((split_b.angular_velocity - b.angular_velocity).length() < 2e-14);
    }

    #[test]
    fn joint_damping_respects_locked_and_dominant_endpoints() {
        let inertia = SolverBodyInertia::new(
            1.0,
            SymmetricMatrix::IDENTITY,
            LockedAxes::new().lock_rotation_x().lock_rotation_z(),
            0,
            true,
        );
        let mut fixed = SolverBody::DUMMY;
        let mut body = SolverBody {
            angular_velocity: DVec3::Y * 2.0,
            ..default()
        };
        let damping = JointDamping {
            linear: 0.0,
            angular: 20.0,
        };
        integrate_joint_damping(
            [&mut fixed, &mut body],
            [&SolverBodyInertia::DUMMY, &inertia],
            [DVec3::ZERO; 2],
            damping,
            1.0 / 480.0,
        )
        .unwrap();
        assert!((body.angular_velocity.y - 2.0 * (-20.0_f64 / 480.0).exp()).abs() < 1e-15);
        assert_eq!(fixed.angular_velocity, DVec3::ZERO);
        assert_eq!(body.angular_velocity.x, 0.0);
        assert_eq!(body.angular_velocity.z, 0.0);

        let dominant =
            SolverBodyInertia::new(1.0, SymmetricMatrix::IDENTITY, LockedAxes::new(), 1, true);
        fixed.angular_velocity = DVec3::Y;
        integrate_joint_damping(
            [&mut fixed, &mut body],
            [&dominant, &inertia],
            [DVec3::ZERO; 2],
            damping,
            1000.0,
        )
        .unwrap();
        assert_eq!(fixed.angular_velocity, DVec3::Y);
        assert_eq!(body.angular_velocity, DVec3::Y);
    }

    #[test]
    fn joint_damping_at_an_offset_point_conserves_total_angular_momentum() {
        let levers = [DVec3::new(1.0, 0.2, 0.4), DVec3::new(-0.3, 0.5, 0.1)];
        let inverse1 = SymmetricMatrix::from_diagonal(DVec3::new(2.0, 3.0, 4.0));
        let inverse2 = SymmetricMatrix::from_diagonal(DVec3::new(5.0, 2.0, 3.0));
        let inertia1 = SolverBodyInertia::new(0.25, inverse1, LockedAxes::new(), 0, true);
        let inertia2 = SolverBodyInertia::new(2.0, inverse2, LockedAxes::new(), 0, true);
        let first = SolverBody {
            linear_velocity: DVec3::new(3.0, -1.0, 2.0),
            angular_velocity: DVec3::new(0.3, 2.0, -0.5),
            ..default()
        };
        let second = SolverBody {
            linear_velocity: DVec3::new(-2.0, 0.5, 7.0),
            angular_velocity: DVec3::new(-1.0, 0.2, 4.0),
            ..default()
        };
        // The common damping point is the origin, so COM positions are -lever.
        let momentum =
            |a: &SolverBody, b: &SolverBody| 4.0 * a.linear_velocity + 0.5 * b.linear_velocity;
        let angular_momentum = |a: &SolverBody, b: &SolverBody| {
            (-levers[0]).cross(4.0 * a.linear_velocity)
                + (-levers[1]).cross(0.5 * b.linear_velocity)
                + inverse1.inverse() * a.angular_velocity
                + inverse2.inverse() * b.angular_velocity
        };
        let energy = |a: &SolverBody, b: &SolverBody| {
            0.5 * (4.0 * a.linear_velocity.length_squared()
                + 0.5 * b.linear_velocity.length_squared()
                + a.angular_velocity
                    .dot(inverse1.inverse() * a.angular_velocity)
                + b.angular_velocity
                    .dot(inverse2.inverse() * b.angular_velocity))
        };
        for angular in [0.0, 11.0] {
            let (mut a, mut b) = (first.clone(), second.clone());
            integrate_joint_damping(
                [&mut a, &mut b],
                [&inertia1, &inertia2],
                levers,
                JointDamping {
                    linear: 7.0,
                    angular,
                },
                0.05,
            )
            .unwrap();
            assert!((momentum(&a, &b) - momentum(&first, &second)).length() < 2e-14);
            assert!(
                (angular_momentum(&a, &b) - angular_momentum(&first, &second)).length() < 2e-14
            );
            assert!(energy(&a, &b) < energy(&first, &second));
            if angular == 0.0 {
                let relative =
                    second.velocity_at_point(levers[1]) - first.velocity_at_point(levers[0]);
                assert!(
                    (b.velocity_at_point(levers[1])
                        - a.velocity_at_point(levers[0])
                        - relative * (-7.0_f64 * 0.05).exp())
                    .length()
                        < 2e-14
                );
            }
        }
        let integrate = |count: u32| {
            let (mut a, mut b) = (first.clone(), second.clone());
            for _ in 0..count {
                integrate_joint_damping(
                    [&mut a, &mut b],
                    [&inertia1, &inertia2],
                    levers,
                    JointDamping {
                        linear: 7.0,
                        angular: 11.0,
                    },
                    0.1 / f64::from(count),
                )
                .unwrap();
            }
            [
                a.linear_velocity,
                b.linear_velocity,
                a.angular_velocity,
                b.angular_velocity,
            ]
        };
        let reference = integrate(1024);
        let error = |count| {
            integrate(count)
                .into_iter()
                .zip(reference)
                .map(|(x, y)| (x - y).length_squared())
                .sum::<f64>()
                .sqrt()
        };
        assert!(
            error(20) < 0.6 * error(10),
            "the split flows must converge under timestep refinement"
        );
    }

    #[test]
    fn joint_damping_point_response_retains_rotated_rank_one_motion() {
        let inertia = SolverBodyInertia::new(
            1.0,
            SymmetricMatrix::IDENTITY,
            LockedAxes::new()
                .lock_translation_x()
                .lock_translation_y()
                .lock_translation_z()
                .lock_rotation_x()
                .lock_rotation_z(),
            0,
            true,
        );
        let mut fixed = SolverBody::DUMMY;
        let mut body = SolverBody {
            angular_velocity: DVec3::Y * 2.0,
            ..default()
        };
        integrate_joint_damping(
            [&mut fixed, &mut body],
            [&SolverBodyInertia::DUMMY, &inertia],
            [DVec3::ZERO, DVec3::new(1.0, 0.3, 2.0)],
            JointDamping {
                linear: 7.0,
                angular: 0.0,
            },
            0.05,
        )
        .unwrap();
        assert!(
            (body.angular_velocity - DVec3::Y * (2.0 * (-7.0_f64 * 0.05).exp())).length() < 2e-14,
            "actual {:?}, expected {:?}",
            body.angular_velocity,
            DVec3::Y * (2.0 * (-7.0_f64 * 0.05).exp())
        );
        assert_eq!(body.linear_velocity, DVec3::ZERO);
        assert_eq!(fixed.angular_velocity, DVec3::ZERO);
        assert_eq!(fixed.linear_velocity, DVec3::ZERO);
    }

    #[test]
    fn joint_damping_rejects_invalid_input_without_partial_velocity_writes() {
        let valid =
            SolverBodyInertia::new(1.0, SymmetricMatrix::IDENTITY, LockedAxes::new(), 0, true);
        let invalid = SolverBodyInertia::new(
            1.0,
            SymmetricMatrix::new(1.0, 2.0, 0.0, 1.0, 0.0, 1.0),
            LockedAxes::new(),
            0,
            true,
        );
        let initial = SolverBody {
            linear_velocity: DVec3::X,
            angular_velocity: DVec3::Y,
            ..default()
        };
        for (rate, dt, inertia) in [
            (f64::NAN, 0.1, &valid),
            (-1.0, 0.1, &valid),
            (1.0, -0.1, &valid),
            (1.0, f64::INFINITY, &valid),
            (1.0, 0.1, &invalid),
        ] {
            let mut first = SolverBody::DUMMY;
            let mut second = initial.clone();
            assert!(
                integrate_joint_damping(
                    [&mut first, &mut second],
                    [&valid, inertia],
                    [DVec3::ZERO; 2],
                    JointDamping {
                        linear: rate,
                        angular: rate
                    },
                    dt
                )
                .is_err()
            );
            assert_eq!(first.linear_velocity, DVec3::ZERO);
            assert_eq!(first.angular_velocity, DVec3::ZERO);
            assert_eq!(second.linear_velocity, initial.linear_velocity);
            assert_eq!(second.angular_velocity, initial.angular_velocity);
        }
    }

    #[test]
    fn bounded_brake_torque_stops_without_reversing_in_one_step() {
        let torque = bounded_brake_torque(100.0, 2.0, 0.25, 0.1);
        assert_eq!(torque, -5.0);

        let torque = bounded_brake_torque(100.0, 2.0, -0.25, 0.1);
        assert_eq!(torque, 5.0);
    }

    #[test]
    fn bounded_brake_torque_respects_authored_limit_away_from_zero() {
        let torque = bounded_brake_torque(100.0, 2.0, 20.0, 0.1);
        assert_eq!(torque, -100.0);
    }

    #[test]
    fn bounded_brake_torque_rejects_invalid_boundary_inputs() {
        for (max_torque, inertia, speed, dt) in [
            (0.0, 2.0, 1.0, 0.1),
            (100.0, 0.0, 1.0, 0.1),
            (100.0, 2.0, f64::NAN, 0.1),
            (100.0, 2.0, 1.0, 0.0),
        ] {
            assert_eq!(bounded_brake_torque(max_torque, inertia, speed, dt), 0.0);
        }
    }
}
